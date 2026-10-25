//! Feeds the visualizer: one worker thread listens (to what the player is playing, or to the
//! microphone), analyzes, and sends the frame for the moment being heard about 60 times a second.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};
use unsub_playback::TapBlock;
use unsub_visualize::{Analyzer, Frame, MicDevice, MicListener};

use crate::error::CommandResult;
use crate::playback::PlaybackState;

/// Sent with the reason when a worker ends by itself, for the UI to say so.
pub const ENDED_EVENT: &str = "visualizer://ended";

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// Audio is held back from the analyzer until it is this near being heard. The analyzer keeps
/// a second of frames and only answers for times within 100 ms of one, so audio queued behind a
/// slow output (AirPlay lags by about two seconds) would otherwise be pushed out of reach.
/// It is also well over the 35 ms a beat needs the audio after it for.
const PUSH_AHEAD: Duration = Duration::from_millis(250);

/// Where the picture comes from.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Source {
    Music,
    Microphone {
        /// A device from `visualizer_mic_devices`; the system default when absent.
        device: Option<String>,
    },
}

/// What a worker hands to whoever listens.
#[derive(Debug)]
enum Output {
    /// Boxed because a frame is far larger than the other message.
    Frame(Box<Frame>),
    /// The source failed for good, and the worker has stopped.
    Ended(String),
}

/// The running worker, if the visualizer is on.
#[derive(Default)]
pub struct VisualizerState {
    worker: Mutex<Option<Worker>>,
}

impl VisualizerState {
    fn worker(&self) -> MutexGuard<'_, Option<Worker>> {
        self.worker.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Replaces any worker with one that calls `next` for each tick and hands what it produces to `send`, until
    /// `send` says the receiver is gone, `next` fails, or the worker is stopped.
    fn start(
        &self,
        next: impl FnMut() -> Result<Option<Frame>, String> + Send + 'static,
        send: impl FnMut(Output) -> bool + Send + 'static,
    ) {
        let mut slot = self.worker();
        // The old worker lets go of its tap or microphone before the new one is running.
        slot.take();
        *slot = Some(Worker::spawn(next, send));
    }

    fn stop(&self) {
        self.worker().take();
    }
}

struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(
        next: impl FnMut() -> Result<Option<Frame>, String> + Send + 'static,
        send: impl FnMut(Output) -> bool + Send + 'static,
    ) -> Worker {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("unsub-visualizer".into())
            .spawn(move || pump(&flag, next, send))
            .expect("the OS can start a thread");
        Worker { stop, thread: Some(thread) }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn pump(stop: &AtomicBool, mut next: impl FnMut() -> Result<Option<Frame>, String>, mut send: impl FnMut(Output) -> bool) {
    let mut due = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match next() {
            Ok(Some(frame)) => {
                if !send(Output::Frame(Box::new(frame))) {
                    return;
                }
            }
            Ok(None) => {}
            Err(reason) => {
                send(Output::Ended(reason));
                return;
            }
        }
        // Counting from the last deadline, not from now, keeps 60 Hz from drifting.
        due += FRAME_INTERVAL;
        thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

/// Chooses what to send from an analyzer's answers: each frame once, however often it is the
/// nearest, and with the beats the frames sent could not know about.
///
/// A frame is marked as a beat only once the frame after it exists, which can be after it has
/// been sent (the mic's newest frame, or music played through a quick output), and at high sample
/// rates some frames are never the nearest at all. So each frame sent also carries any beat
/// marked on the frames since the last one, up to the newest frame that can still change.
struct Feeder {
    sent: Option<u64>,
    beats_through: Duration,
}

impl Feeder {
    fn new() -> Feeder {
        Feeder { sent: None, beats_through: Duration::ZERO }
    }

    fn next(&mut self, analyzer: &Analyzer, now: Duration) -> Option<Frame> {
        let mut frame = analyzer.frame_at(now)?;
        if self.sent == Some(frame.t_ms) {
            return None;
        }
        self.sent = Some(frame.t_ms);
        let upto = analyzer.beats_known_until().unwrap_or_default().min(Duration::from_millis(frame.t_ms));
        if let Some(strength) = analyzer.beat_between(self.beats_through, upto) {
            frame.beat = true;
            frame.beat_strength = frame.beat_strength.max(strength);
        }
        self.beats_through = self.beats_through.max(upto);
        Some(frame)
    }
}

/// A block of what the player is playing, kept until it is nearly heard.
struct Block {
    sample_rate: u32,
    channels: u16,
    at: Duration,
    samples: Vec<f32>,
}

impl From<TapBlock<'_>> for Block {
    fn from(block: TapBlock<'_>) -> Block {
        Block { sample_rate: block.sample_rate, channels: block.channels, at: block.at, samples: block.samples.to_vec() }
    }
}

/// The music's side of the analysis.
struct Music {
    waiting: VecDeque<Block>,
    analyzer: Analyzer,
    feeder: Feeder,
}

impl Music {
    fn new() -> Music {
        Music { waiting: VecDeque::new(), analyzer: Analyzer::new(44_100, 2), feeder: Feeder::new() }
    }

    fn accept(&mut self, block: Block) {
        self.waiting.push_back(block);
    }

    /// The frame to send for `now` on the clock the blocks are stamped with, if there is a new one.
    fn frame(&mut self, now: Duration) -> Option<Frame> {
        while let Some(block) = self.waiting.pop_front_if(|block| block.at <= now + PUSH_AHEAD) {
            self.analyzer.reconfigure(block.sample_rate, block.channels);
            self.analyzer.push(&block.samples, block.at);
        }
        self.feeder.next(&self.analyzer, now)
    }
}

fn sender(app: AppHandle, on_frame: Channel<Frame>) -> impl FnMut(Output) -> bool + Send + 'static {
    move |output| match output {
        Output::Frame(frame) => on_frame.send(*frame).is_ok(),
        Output::Ended(reason) => {
            let _ = app.emit(ENDED_EVENT, reason);
            false
        }
    }
}

/// Starts (or restarts) the picture's feed. The microphone is opened only here, and only when asked for.
#[tauri::command]
pub async fn visualizer_start(
    app: AppHandle,
    visualizer: State<'_, VisualizerState>,
    playback: State<'_, PlaybackState>,
    on_frame: Channel<Frame>,
    source: Source,
) -> CommandResult<()> {
    // Let go of the old source first: a microphone can be held by one listener at a time.
    visualizer.stop();
    match source {
        Source::Music => {
            let mut tap = playback.player.tap();
            let mut music = Music::new();
            visualizer.start(
                move || {
                    while let Some(block) = tap.read() {
                        music.accept(block.into());
                    }
                    Ok(music.frame(tap.now()))
                },
                sender(app, on_frame),
            );
        }
        Source::Microphone { device } => {
            permission::ensure_microphone_access()?;
            let analyzer = Arc::new(Mutex::new(Analyzer::new(48_000, 1)));
            let listener = MicListener::start(device, Arc::clone(&analyzer))?;
            let mut feeder = Feeder::new();
            visualizer.start(
                move || {
                    if !listener.is_running() {
                        return Err("The microphone stopped, so the visualizer is listening to the music again.".to_owned());
                    }
                    let analyzer = analyzer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    Ok(feeder.next(&analyzer, listener.now()))
                },
                sender(app, on_frame),
            );
        }
    }
    Ok(())
}

/// Async so that waiting for the worker to finish happens off the main thread.
#[tauri::command]
pub async fn visualizer_stop(visualizer: State<'_, VisualizerState>) -> CommandResult<()> {
    visualizer.stop();
    Ok(())
}

/// Listing microphones does not open one, so it asks for no permission.
#[tauri::command]
pub async fn visualizer_mic_devices() -> CommandResult<Vec<MicDevice>> {
    Ok(MicListener::devices()?)
}

/// macOS gives a refused microphone stream silence rather than an error, so access is checked
/// first, and asked for if the user has not been asked yet. Nothing here runs unless the
/// microphone was chosen.
mod permission {
    use crate::error::CommandResult;

    #[cfg(target_os = "macos")]
    pub fn ensure_microphone_access() -> CommandResult<()> {
        check_microphone_access(objc2::available!(macos = 10.14), macos::authorization_status, macos::request_access)
    }

    #[cfg(target_os = "macos")]
    fn check_microphone_access(
        has_privacy_controls: bool,
        status: impl FnOnce() -> isize,
        request: impl FnOnce() -> bool,
    ) -> CommandResult<()> {
        use crate::error::CommandError;

        // These selectors were introduced with microphone privacy controls in Mojave.
        // High Sierra can open CoreAudio input directly; sending either selector there crashes.
        if !has_privacy_controls {
            return Ok(());
        }

        match status() {
            macos::AUTHORIZED => Ok(()),
            macos::NOT_DETERMINED if request() => Ok(()),
            _ => Err(CommandError::from(
                "Microphone access is off for Unsub. Enable it in the microphone privacy settings in System Settings or System Preferences.".to_owned(),
            )),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn ensure_microphone_access() -> CommandResult<()> {
        Ok(())
    }

    #[cfg(all(test, target_os = "macos"))]
    mod tests {
        use super::*;

        #[test]
        fn high_sierra_does_not_send_mojave_permission_selectors() {
            assert!(
                check_microphone_access(false, || panic!("status selector unavailable"), || panic!("request selector unavailable")).is_ok()
            );
        }

        #[test]
        fn authorized_input_never_prompts_again() {
            assert!(check_microphone_access(true, || macos::AUTHORIZED, || panic!("already authorized")).is_ok());
        }

        #[test]
        fn modern_microphone_denials_still_fail_closed() {
            assert!(check_microphone_access(true, || 2, || panic!("must not prompt after denial")).is_err());
            assert!(check_microphone_access(true, || macos::NOT_DETERMINED, || false).is_err());
            assert!(check_microphone_access(true, || macos::NOT_DETERMINED, || true).is_ok());
        }
    }

    /// AVFoundation's `AVCaptureDevice`, declared here straight from the SDK headers as the
    /// applemusic crate does for its framework: it is not worth a generated bindings crate.
    #[cfg(target_os = "macos")]
    mod macos {
        use std::sync::mpsc;

        use block2::{DynBlock, RcBlock};
        use objc2::runtime::{Bool, NSObject};
        use objc2::{extern_class, extern_methods};
        use objc2_foundation::NSString;

        // Raw values of AVAuthorizationStatus in AVCaptureDevice.h.
        pub const NOT_DETERMINED: isize = 0;
        pub const AUTHORIZED: isize = 3;

        #[link(name = "AVFoundation", kind = "framework")]
        unsafe extern "C" {
            // SAFETY: AVMediaFormat.h declares `AVMediaTypeAudio` as a constant `NSString *`.
            static AVMediaTypeAudio: &'static NSString;
        }

        extern_class!(
            // SAFETY: AVCaptureDevice.h declares `AVCaptureDevice : NSObject`.
            #[unsafe(super(NSObject))]
            struct AVCaptureDevice;
        );

        impl AVCaptureDevice {
            extern_methods!(
                // SAFETY: selectors and types are copied from AVCaptureDevice.h:
                // `+authorizationStatusForMediaType:` returns an NSInteger enum, and
                // `+requestAccessForMediaType:completionHandler:` takes a `void (^)(BOOL)` that
                // AVFoundation may call on any queue.
                #[unsafe(method(authorizationStatusForMediaType:))]
                fn authorization_status_for(media_type: &NSString) -> isize;

                #[unsafe(method(requestAccessForMediaType:completionHandler:))]
                fn request_access_for(media_type: &NSString, handler: &DynBlock<dyn Fn(Bool)>);
            );
        }

        pub fn authorization_status() -> isize {
            // SAFETY: the constant is set when the framework loads, and is never written.
            AVCaptureDevice::authorization_status_for(unsafe { AVMediaTypeAudio })
        }

        /// Shows the system's prompt and waits for the answer.
        pub fn request_access() -> bool {
            let (answer, answered) = mpsc::channel();
            let handler = RcBlock::new(move |granted: Bool| {
                let _ = answer.send(granted.as_bool());
            });
            // SAFETY: as above.
            AVCaptureDevice::request_access_for(unsafe { AVMediaTypeAudio }, &handler);
            answered.recv().unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use unsub_visualize::{BANDS, SCOPE_POINTS};

    use super::*;

    const PATIENCE: Duration = Duration::from_secs(5);

    fn frame(t_ms: u64) -> Frame {
        Frame {
            t_ms,
            bands: [0.0; BANDS],
            peaks: [0.0; BANDS],
            waveform_l: [0.0; SCOPE_POINTS],
            waveform_r: [0.0; SCOPE_POINTS],
            rms: 0.0,
            beat: false,
            beat_strength: 0.0,
            onset: 0.0,
            tempo_bpm: None,
        }
    }

    fn secs(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    /// Counts how many of it have been dropped, which is how a test sees a worker let go of what it held.
    struct Held(Arc<AtomicUsize>);

    impl Drop for Held {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// A worker's output, as a test can compare it.
    fn seen(output: Output) -> Result<u64, String> {
        match output {
            Output::Frame(frame) => Ok(frame.t_ms),
            Output::Ended(reason) => Err(reason),
        }
    }

    #[test]
    fn each_frame_is_offered_once_however_often_it_is_the_nearest() {
        let tone: Vec<f32> = (0..48_000).map(|n| 0.5 * (TAU * 440.0 * n as f32 / 48_000.0).sin()).collect();
        let mut analyzer = Analyzer::new(48_000, 1);
        analyzer.push(&tone, Duration::ZERO);
        let mut feeder = Feeder::new();

        let first = feeder.next(&analyzer, secs(0.5)).expect("a frame");
        assert!(feeder.next(&analyzer, secs(0.5)).is_none(), "the same frame again");
        assert!(feeder.next(&analyzer, secs(0.5005)).is_none());
        let later = feeder.next(&analyzer, secs(0.6)).expect("a newer frame");

        assert!(later.t_ms > first.t_ms);
    }

    /// A kick drum at each of `beats` (seconds) in a stereo track.
    fn kick_track(rate: u32, seconds: f64, beats: &[f64]) -> Vec<f32> {
        let mut track = vec![0.0_f32; (seconds * f64::from(rate)) as usize * 2];
        for &beat in beats {
            let start = (beat * f64::from(rate)) as usize;
            for n in 0..(0.12 * rate as f32) as usize {
                let t = n as f32 / rate as f32;
                let sample = 0.9 * (TAU * 55.0 * t).sin() * (-t * 30.0).exp();
                if let Some(pair) = track.get_mut((start + n) * 2..(start + n) * 2 + 2) {
                    pair.fill(sample);
                }
            }
        }
        track
    }

    /// Plays `track` as the player would hand it over: the output renders a block of 512 frames
    /// every 512/rate seconds, each heard `latency` after it is rendered, and the visualizer ticks
    /// 60 times a second, reading whatever has been rendered. Returns each tick's time and the
    /// frame sent, if one was.
    fn play(track: &[f32], rate: u32, latency: Duration, seconds: f64) -> Vec<(Duration, Frame)> {
        let mut music = Music::new();
        let mut blocks = track.chunks(512 * 2).enumerate().peekable();
        let mut sent = Vec::new();
        for tick in 0.. {
            let now = FRAME_INTERVAL * tick;
            if now.as_secs_f64() > seconds {
                return sent;
            }
            while let Some((index, chunk)) = blocks.next_if(|(index, _)| secs((index * 512) as f64 / f64::from(rate)) <= now) {
                let rendered = secs((index * 512) as f64 / f64::from(rate));
                music.accept(Block { sample_rate: rate, channels: 2, at: rendered + latency, samples: chunk.to_vec() });
            }
            if let Some(frame) = music.frame(now) {
                sent.push((now, frame));
            }
        }
        unreachable!("the loop ends by returning")
    }

    #[test]
    fn every_beat_is_sent_once_whatever_the_latency_and_sample_rate() {
        let clicks: Vec<f64> = (0..).map(|n| 0.25 + 0.5 * f64::from(n)).take_while(|&t| t < 11.0).collect();
        // Quick outputs know a beat only after the frame has been sent; the microphone is no
        // latency at all; 96 kHz makes more frames than there are ticks; AirPlay lags two seconds.
        for (rate, latency) in [(48_000, 10), (48_000, 0), (96_000, 10), (44_100, 30), (48_000, 2_000)] {
            let latency = Duration::from_millis(latency);
            let track = kick_track(rate, 12.0, &clicks);
            let sent = play(&track, rate, latency, 12.0 + latency.as_secs_f64() + 0.5);
            let beats: Vec<f64> = sent.iter().filter(|(_, frame)| frame.beat).map(|(_, frame)| frame.t_ms as f64 / 1000.0).collect();
            let label = format!("{rate} Hz, {latency:?}");

            assert!(sent.len() > 400, "{label}: {} frames", sent.len());
            // A beat is only recognised once about 35 ms of the music after it has been heard, and
            // goes out on the next frame, so it can land that much after the click, never before.
            let lands = |beat: f64, click: f64| (-0.02..=0.08).contains(&(beat - click - latency.as_secs_f64()));
            for &click in clicks.iter().filter(|&&click| click > 2.0) {
                let heard = click + latency.as_secs_f64();
                let near = beats.iter().filter(|&&beat| lands(beat, click)).count();
                assert_eq!(near, 1, "{label}: {near} beats for the click heard at {heard} s, in {beats:?}");
            }
            let stray =
                beats.iter().filter(|&&beat| beat > 2.5 + latency.as_secs_f64() && clicks.iter().all(|&click| !lands(beat, click))).count();
            assert_eq!(stray, 0, "{label}: beats off the clicks in {beats:?}");
            // A frame is for the moment being heard, however far ahead the audio is queued.
            for (now, frame) in &sent {
                let late = (frame.t_ms as f64 / 1000.0 - now.as_secs_f64()).abs();
                assert!(late <= 0.1, "{label}: at {now:?} the frame was {late} s away");
            }
        }
    }

    #[test]
    fn no_frame_means_nothing_is_sent() {
        let state = VisualizerState::default();
        let (sent, received) = mpsc::channel();
        state.start(|| Ok(None), move |output| sent.send(seen(output)).is_ok());

        assert!(received.recv_timeout(Duration::from_millis(150)).is_err());
        state.stop();
    }

    #[test]
    fn frames_are_sent_as_they_come() {
        let state = VisualizerState::default();
        let (sent, received) = mpsc::channel();
        let mut t_ms = 0;
        state.start(
            move || {
                t_ms += 1;
                Ok(Some(frame(t_ms)))
            },
            move |output| sent.send(seen(output)).is_ok(),
        );

        let times: Vec<_> = (0..4).map(|_| received.recv_timeout(PATIENCE).unwrap()).collect();
        state.stop();

        assert_eq!(times, [Ok(1), Ok(2), Ok(3), Ok(4)]);
    }

    #[test]
    fn a_source_that_fails_ends_the_worker_and_says_why() {
        let state = VisualizerState::default();
        let (sent, received) = mpsc::channel();
        let dropped = Arc::new(AtomicUsize::new(0));
        let held = Held(Arc::clone(&dropped));
        state.start(
            move || {
                let _ = &held;
                Err("the microphone was unplugged".to_owned())
            },
            move |output| sent.send(seen(output)).is_ok(),
        );

        assert_eq!(received.recv_timeout(PATIENCE).unwrap(), Err("the microphone was unplugged".to_owned()));
        wait_for("the worker to end", || dropped.load(Ordering::SeqCst) == 1);
        assert!(received.try_recv().is_err(), "nothing after the end");
        state.stop();
    }

    #[test]
    fn starting_again_replaces_the_worker_and_lets_go_of_what_it_held() {
        let state = VisualizerState::default();
        let dropped = Arc::new(AtomicUsize::new(0));
        let held = Held(Arc::clone(&dropped));
        state.start(
            move || {
                let _ = &held;
                Ok(None)
            },
            |_| true,
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 0);

        let (sent, received) = mpsc::channel();
        state.start(|| Ok(Some(frame(7))), move |output| sent.send(seen(output)).is_ok());

        assert_eq!(dropped.load(Ordering::SeqCst), 1, "the first worker is gone before start returns");
        assert_eq!(received.recv_timeout(PATIENCE).unwrap(), Ok(7));
        state.stop();
    }

    #[test]
    fn stopping_is_safe_whether_or_not_anything_runs_and_lets_go_of_what_the_worker_held() {
        let state = VisualizerState::default();
        state.stop();

        let dropped = Arc::new(AtomicUsize::new(0));
        let held = Held(Arc::clone(&dropped));
        state.start(
            move || {
                let _ = &held;
                Ok(None)
            },
            |_| true,
        );
        state.stop();
        state.stop();

        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_worker_whose_receiver_has_gone_ends_by_itself() {
        let state = VisualizerState::default();
        let dropped = Arc::new(AtomicUsize::new(0));
        let held = Held(Arc::clone(&dropped));
        state.start(
            move || {
                let _ = &held;
                Ok(Some(frame(1)))
            },
            |_| false,
        );

        wait_for("the worker to end", || dropped.load(Ordering::SeqCst) == 1);
        state.stop();
    }

    #[test]
    fn sources_are_read_as_the_ui_writes_them() {
        let read = |json: &str| serde_json::from_str::<Source>(json).unwrap();
        assert!(matches!(read(r#"{"type":"music"}"#), Source::Music));
        assert!(matches!(read(r#"{"type":"microphone"}"#), Source::Microphone { device: None }));
        assert!(matches!(read(r#"{"type":"microphone","device":null}"#), Source::Microphone { device: None }));
        let Source::Microphone { device } = read(r#"{"type":"microphone","device":"usb-1"}"#) else { panic!("not the microphone") };
        assert_eq!(device.as_deref(), Some("usb-1"));
    }
}
