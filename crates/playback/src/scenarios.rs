//! The whole player, device included, run against a simulated audio system: a thread that drains
//! the ring as hardware would and records what it was given.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::host::simulated::{Config, Log, Simulated};
use crate::{Decoder, PlaybackState, Player, PlayerEvent};

/// How long to wait for anything to happen. Waits return the moment their condition holds, so this
/// only has to outlast a machine that is busy with a great deal else.
const PATIENCE: Duration = Duration::from_secs(20);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn decode(name: &str) -> Vec<f32> {
    let mut decoder = Decoder::open(&fixture(name)).unwrap();
    let mut samples = Vec::new();
    while let Some(chunk) = decoder.next_chunk().unwrap() {
        samples.extend_from_slice(chunk);
    }
    samples
}

struct Rig {
    player: Player,
    events: Receiver<PlayerEvent>,
    seen: Vec<PlayerEvent>,
    log: Arc<Mutex<Log>>,
}

fn rig(config: Config) -> Rig {
    let (sender, events) = mpsc::channel();
    let (host, log) = Simulated::new(config);
    let player = Player::with_host(
        move |event| {
            let _ = sender.send(event);
        },
        move || Box::new(host),
    )
    .unwrap();
    Rig {
        player,
        events,
        seen: Vec::new(),
        log,
    }
}

impl Rig {
    /// Reads events until one satisfies `wanted`, and returns it.
    fn until(&mut self, what: &str, wanted: impl Fn(&PlayerEvent) -> bool) -> PlayerEvent {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) => {
                    self.seen.push(event.clone());
                    if wanted(&event) {
                        return event;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("timed out waiting for {what}: {:?}", self.seen)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("engine ended while waiting for {what}")
                }
            }
        }
    }

    fn until_stopped(&mut self) {
        self.until("playback to stop", |event| {
            matches!(event, PlayerEvent::StateChanged(status) if status.state == PlaybackState::Stopped)
        });
    }

    fn until_playing(&mut self, track_id: i64) {
        self.until("the track to play", |event| {
            matches!(event, PlayerEvent::StateChanged(status)
                if status.state == PlaybackState::Playing && status.track_id == Some(track_id))
        });
    }

    /// Everything seen so far plus anything still arriving within a short while.
    fn settle(&mut self) -> &[PlayerEvent] {
        while let Ok(event) = self.events.recv_timeout(Duration::from_millis(150)) {
            self.seen.push(event);
        }
        &self.seen
    }

    fn notices(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::Notice { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    fn heard(&self) -> Vec<f32> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .heard
            .clone()
    }

    fn opened(&self) -> Vec<(u32, bool)> {
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        log.opened
            .iter()
            .map(|request| (request.source.sample_rate, request.exact))
            .collect()
    }

    fn device_calls(&self) -> Vec<&'static str> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .device
            .clone()
    }

    /// Polls the status until `check` holds.
    fn await_status(&self, what: &str, check: impl Fn(&crate::PlaybackStatus) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !check(&self.player.status()) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Where `needle` first occurs in `haystack`.
fn find(haystack: &[f32], needle: &[f32]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Asserts the device was given `source[from..]` back to back, apart from the fade-in at the
/// start and fade-out at the end, which are `margin` samples wide.
fn assert_played(heard: &[f32], source: &[f32], from: usize, margin: usize) {
    let probe = from + margin + 400;
    let at = find(heard, &source[probe..probe + 256])
        .unwrap_or_else(|| panic!("the source never reached the device"));
    let start = at - (probe - from);
    let end = source.len() - from;
    assert!(
        heard[start + margin..start + end - margin] == source[from + margin..source.len() - margin],
        "the device did not receive the source unchanged"
    );
}

#[test]
fn a_file_plays_through_to_the_end_and_the_device_receives_it_unchanged() {
    let mut rig = rig(Config::default());
    rig.player.play(1, fixture("sine.flac"));

    rig.until("the track to end", |e| {
        matches!(e, PlayerEvent::TrackEnded { track_id: 1 })
    });
    rig.until_stopped();

    assert_played(&rig.heard(), &decode("sine.flac"), 0, 2_000);
    assert_eq!(rig.opened(), [(44_100, false)]);
}

#[test]
fn gapless_tracks_reach_the_device_back_to_back_through_one_stream() {
    let mut rig = rig(Config::default());
    rig.player.play(1, fixture("sine.flac"));
    rig.player.queue_next(2, fixture("sine.mp3"));

    rig.until("the second track to start", |e| {
        matches!(e, PlayerEvent::TrackStarted { track_id: 2 })
    });
    rig.until_stopped();

    let both = [decode("sine.flac"), decode("sine.mp3")].concat();
    assert_played(&rig.heard(), &both, 0, 2_000);
    assert_eq!(rig.opened().len(), 1, "no reopen between gapless tracks");
    let order: Vec<_> = rig
        .seen
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::TrackEnded { track_id } => Some(("ended", *track_id)),
            PlayerEvent::TrackStarted { track_id } => Some(("started", *track_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [("started", 1), ("ended", 1), ("started", 2), ("ended", 2)]
    );
}

#[test]
fn seeking_resumes_from_exactly_the_requested_frame() {
    let mut rig = rig(Config {
        speed: 1,
        ..Config::default()
    });
    rig.player.play(1, fixture("sine.flac"));
    rig.until_playing(1);

    rig.player.seek(700);
    rig.until("the track to end", |e| {
        matches!(e, PlayerEvent::TrackEnded { track_id: 1 })
    });

    // The tail from 700 ms (frame 30870) reaches the device whole.
    let source = decode("sine.flac");
    let heard = rig.heard();
    let tail = &source[2 * 30_870..];
    let at = find(&heard, &tail[4_000..4_256]).expect("the tail was played");
    let start = at - 4_000;
    assert!(heard[start + 2_000..start + tail.len() - 2_000] == tail[2_000..tail.len() - 2_000]);
}

#[test]
fn pause_holds_the_position_and_resume_continues() {
    let mut rig = rig(Config {
        speed: 1,
        ..Config::default()
    });
    rig.player.play(1, fixture("sine.flac"));
    rig.until_playing(1);
    rig.await_status("some progress", |s| s.position_ms > 100);

    rig.player.pause();
    rig.await_status("pause", |s| s.state == PlaybackState::Paused);
    // The fade-out and the next status refresh both take a few milliseconds.
    std::thread::sleep(Duration::from_millis(100));
    let held = rig.player.status().position_ms;
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        rig.player.status().position_ms,
        held,
        "paused audio does not advance"
    );

    rig.player.resume();
    rig.until("the track to end", |e| {
        matches!(e, PlayerEvent::TrackEnded { track_id: 1 })
    });
}

mod bit_perfect {
    use super::*;

    fn exact_rig() -> Rig {
        rig(Config::default())
    }

    #[test]
    fn plays_the_source_unchanged_at_unity_whatever_the_volume_and_says_so() {
        let mut rig = exact_rig();
        rig.player.set_volume(0.3);
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));

        rig.until_playing(1);
        let status = rig.player.status();
        assert!(status.bit_perfect);
        assert_eq!(status.volume, 1.0, "volume is fixed at unity");
        assert_eq!(
            status.output_format.as_deref(),
            Some("32-bit float 44.1 kHz")
        );

        rig.until_stopped();
        assert_played(&rig.heard(), &decode("sine.flac"), 0, 2_000);
        assert_eq!(rig.opened(), [(44_100, true)]);
        assert_eq!(rig.device_calls(), ["capture", "take_exclusive", "restore"]);
        assert!(
            !rig.player.status().bit_perfect,
            "nothing is playing any more"
        );
    }

    #[test]
    fn without_it_the_volume_applies_and_the_device_is_not_taken() {
        let mut rig = exact_rig();
        rig.player.set_volume(0.3);
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        let status = rig.player.status();
        assert!(!status.bit_perfect);
        assert_eq!(status.volume, 0.3);

        rig.until_stopped();
        let source = decode("sine.flac");
        let peak = rig
            .heard()
            .iter()
            .fold(0.0_f32, |peak, s| peak.max(s.abs()));
        let source_peak = source.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak < 0.05 * source_peak,
            "cubic volume 0.3 is about -31 dB"
        );
        assert_eq!(rig.device_calls(), ["capture", "restore"]);
    }

    #[test]
    fn changing_the_volume_while_it_is_on_explains_that_the_device_is_in_charge() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.await_status("bit-perfect", |s| s.bit_perfect);

        rig.player.set_volume(0.5);
        let notice = rig.until("a notice", |e| matches!(e, PlayerEvent::Notice { .. }));

        assert!(
            matches!(&notice, PlayerEvent::Notice { message } if message.contains("device")),
            "{notice:?}"
        );
        assert_eq!(rig.player.status().volume, 1.0);
    }

    #[test]
    fn changing_the_volume_normally_says_nothing() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.player.set_volume(0.5);
        rig.settle();
        assert!(rig.notices().is_empty());
        assert_eq!(rig.player.status().volume, 0.5);
    }

    #[test]
    fn a_rate_the_device_lacks_plays_normally_with_a_notice_and_gives_the_device_back() {
        let mut rig = exact_rig();
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine-22k.aiff"));

        rig.until_playing(1);
        let status = rig.player.status();
        assert!(!status.bit_perfect);
        assert_eq!(
            status.output_sample_rate,
            Some(48_000),
            "resampled to the device default"
        );
        rig.until_stopped();

        let notices = rig.notices();
        assert!(
            notices.iter().any(|m| m.contains("22.05 kHz")),
            "the notice names the rate: {notices:?}"
        );
        assert_eq!(
            rig.device_calls(),
            ["capture", "take_exclusive", "release_exclusive", "restore"],
            "exclusivity is dropped as soon as it is known to be useless"
        );
        assert_eq!(rig.opened(), [(22_050, false)]);
    }

    #[test]
    fn a_device_held_by_another_app_is_not_forced() {
        let mut rig = rig(Config {
            held_elsewhere: true,
            ..Config::default()
        });
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));

        rig.until_playing(1);
        assert!(!rig.player.status().bit_perfect);
        rig.until_stopped();
        assert!(
            rig.notices().iter().any(|m| m.contains("another app")),
            "{:?}",
            rig.notices()
        );
        assert_eq!(rig.opened(), [(44_100, false)]);
    }

    #[test]
    fn asking_while_nothing_plays_opens_nothing_until_there_is_a_track() {
        let mut rig = exact_rig();
        rig.player.set_bit_perfect(true);
        rig.settle();
        assert!(rig.opened().is_empty());
        assert!(rig.device_calls().is_empty());

        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        assert_eq!(rig.opened(), [(44_100, true)]);
    }

    #[test]
    fn switching_on_mid_track_moves_the_audio_to_an_exact_stream_where_it_was() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.await_status("some progress", |s| s.position_ms > 200);

        rig.player.set_bit_perfect(true);
        rig.await_status("bit-perfect", |s| s.bit_perfect);
        let resumed = rig.player.status().position_ms;
        assert!(
            (150..600).contains(&resumed),
            "carried on from about where it was: {resumed} ms"
        );

        rig.until("the track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 1 })
        });
        rig.until_stopped();
        assert_eq!(rig.opened(), [(44_100, false), (44_100, true)]);
        let started = rig
            .seen
            .iter()
            .filter(|e| matches!(e, PlayerEvent::TrackStarted { .. }))
            .count();
        let ended = rig
            .seen
            .iter()
            .filter(|e| matches!(e, PlayerEvent::TrackEnded { .. }))
            .count();
        assert_eq!((started, ended), (1, 1), "the same track carries on");
        assert_eq!(rig.device_calls(), ["capture", "take_exclusive", "restore"]);
    }

    #[test]
    fn switching_off_mid_track_returns_to_the_shared_device_and_the_volume() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.await_status("bit-perfect", |s| s.bit_perfect);
        rig.player.set_volume(0.5);

        rig.player.set_bit_perfect(false);
        rig.await_status("shared mode", |s| {
            !s.bit_perfect && s.state == PlaybackState::Playing
        });
        assert_eq!(
            rig.player.status().volume,
            0.5,
            "the remembered volume applies again"
        );

        rig.until("the track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 1 })
        });
        rig.until_stopped();
        assert_eq!(rig.opened(), [(44_100, true), (44_100, false)]);
        assert_eq!(
            rig.device_calls(),
            ["capture", "take_exclusive", "release_exclusive", "restore"]
        );
    }

    #[test]
    fn switching_while_paused_stays_paused() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.player.pause();
        rig.await_status("pause", |s| s.state == PlaybackState::Paused);
        let held = rig.player.status().position_ms;

        rig.player.set_bit_perfect(true);
        rig.await_status("bit-perfect", |s| s.bit_perfect);
        std::thread::sleep(Duration::from_millis(150));

        let status = rig.player.status();
        assert_eq!(status.state, PlaybackState::Paused);
        assert!(
            status.position_ms.abs_diff(held) < 50,
            "{} vs {held}",
            status.position_ms
        );
    }

    #[test]
    fn a_run_of_tracks_takes_the_device_once_and_gives_it_back_once() {
        let mut rig = rig(Config::default());
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));
        rig.player.queue_next(2, fixture("sine.flac"));
        rig.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        rig.until_stopped();

        assert_eq!(
            rig.opened(),
            [(44_100, true)],
            "gapless tracks share the stream"
        );
        assert_eq!(rig.device_calls(), ["capture", "take_exclusive", "restore"]);
    }

    #[test]
    fn a_new_rate_in_the_same_run_reopens_the_stream_but_keeps_the_device() {
        let mut rig = rig(Config::default());
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));
        rig.player.queue_next(2, fixture("sine-alac.m4a"));
        rig.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        rig.until_stopped();

        assert_eq!(rig.opened(), [(44_100, true), (48_000, true)]);
        let calls = rig.device_calls();
        assert_eq!(calls.iter().filter(|c| **c == "capture").count(), 1);
        assert_eq!(calls.iter().filter(|c| **c == "restore").count(), 1);
    }

    #[test]
    fn a_change_of_bit_depth_needs_a_new_stream_only_when_exact() {
        let mut exact = rig(Config::default());
        exact.player.set_bit_perfect(true);
        exact.player.play(1, fixture("sine.flac"));
        exact.player.queue_next(2, fixture("sine-24bit.flac"));
        exact.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        assert_eq!(exact.opened(), [(44_100, true), (44_100, true)]);

        let mut shared = rig(Config::default());
        shared.player.play(1, fixture("sine.flac"));
        shared.player.queue_next(2, fixture("sine-24bit.flac"));
        shared.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        assert_eq!(
            shared.opened(),
            [(44_100, false)],
            "volume mode stays gapless across depths"
        );
    }

    #[test]
    fn the_fallback_is_retried_for_the_next_track() {
        let mut rig = rig(Config {
            rates: vec![44_100],
            ..Config::default()
        });
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine-22k.aiff"));
        rig.player.queue_next(2, fixture("sine.flac"));
        rig.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        rig.until_stopped();

        assert_eq!(rig.opened(), [(22_050, false), (44_100, true)]);
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn dsd_is_converted_and_reported_as_not_bit_perfect() {
        let mut rig = exact_rig();
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine-dsd64.dsf"));
        rig.until_playing(1);
        assert!(!rig.player.status().bit_perfect);
        rig.until_stopped();
        assert!(
            rig.notices().iter().any(|m| m.contains("352.8 kHz")),
            "{:?}",
            rig.notices()
        );
    }
}

mod sound_check {
    use super::*;
    use crate::Gain;

    /// The fixtures are 440 Hz sines peaking at -6.02 dBFS.
    const QUIET: Gain = Gain {
        db: -6.0,
        peak_dbtp: -6.02,
    };
    const LOUD: Gain = Gain {
        db: 3.0,
        peak_dbtp: -6.4,
    };

    fn scaled(name: &str, gain: Gain) -> Vec<f32> {
        let amplitude = gain.amplitude();
        decode(name)
            .iter()
            .map(|sample| sample * amplitude)
            .collect()
    }

    #[test]
    fn the_device_receives_each_track_at_its_own_gain_with_the_change_on_the_boundary() {
        let mut rig = rig(Config::default());
        rig.player.play_with(1, fixture("sine.flac"), Some(QUIET));
        rig.player
            .queue_next_with(2, fixture("sine.mp3"), Some(LOUD));
        rig.until_stopped();

        let both = [scaled("sine.flac", QUIET), scaled("sine.mp3", LOUD)].concat();
        assert_played(&rig.heard(), &both, 0, 2_000);
        assert_eq!(rig.opened().len(), 1, "the gain change keeps the stream");
    }

    #[test]
    fn the_status_follows_the_audible_track_across_a_gapless_boundary() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play_with(1, fixture("sine.flac"), Some(QUIET));
        rig.player
            .queue_next_with(2, fixture("sine.mp3"), Some(LOUD));

        rig.await_status("the first track's gain", |s| {
            s.track_id == Some(1) && s.gain_db == Some(-6.0)
        });
        rig.await_status("the second track's gain", |s| {
            s.track_id == Some(2) && s.gain_db == Some(3.0)
        });
        rig.until_stopped();
        assert_eq!(rig.player.status().gain_db, None);
    }

    #[test]
    fn a_boost_is_held_back_so_the_device_never_sees_more_than_minus_one_dbtp() {
        let mut rig = rig(Config::default());
        let greedy = Gain {
            db: 12.0,
            peak_dbtp: -6.02,
        };
        rig.player.play_with(1, fixture("sine.flac"), Some(greedy));
        rig.until_playing(1);
        let applied = rig.player.status().gain_db.expect("a gain is applied");
        assert!((applied - 5.02).abs() < 1e-4, "{applied}");

        rig.until_stopped();
        let peak = rig
            .heard()
            .iter()
            .fold(0.0_f32, |peak, s| peak.max(s.abs()));
        let ceiling = 10f32.powf(-1.0 / 20.0);
        assert!(
            (peak - ceiling).abs() < 1e-3,
            "peak {peak}, ceiling {ceiling}"
        );
    }

    #[test]
    fn a_seek_while_the_next_track_is_already_decoded_keeps_the_playing_tracks_gain() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play_with(1, fixture("sine.flac"), Some(QUIET));
        rig.player
            .queue_next_with(2, fixture("sine.mp3"), Some(LOUD));
        // The ring holds half a second, so by now the whole first track is decoded and the
        // second has been handed on to, with its louder gain, ahead of what is heard.
        rig.await_status("most of the first track", |s| s.position_ms >= 650);

        rig.player.seek(300);
        rig.until_stopped();

        let tail = [
            scaled("sine.flac", QUIET)[2 * 13_230..].to_vec(),
            scaled("sine.mp3", LOUD),
        ]
        .concat();
        // The sine repeats every 2205 frames, so the end of the first track is told apart from
        // what was heard before the seek by running on into the second track. From frame 40000
        // of the first track on, everything was only ever heard after the seek.
        let late = 2 * (40_000 - 13_230);
        let join = 2 * (44_100 - 13_230);
        let heard = rig.heard();
        let at = find(&heard, &tail[join - 256..join + 256]).expect("the first track running on");
        let start = at - (join - 256 - late);
        let rest = &tail[late..];
        assert!(
            heard[start..start + rest.len() - 2_000] == rest[..rest.len() - 2_000],
            "the rest of the first track is at its own gain and the second follows at its own"
        );
    }

    #[test]
    fn bit_perfect_plays_the_file_untouched_and_reports_no_gain() {
        let mut rig = rig(Config::default());
        rig.player.set_bit_perfect(true);
        rig.player.play_with(1, fixture("sine.flac"), Some(QUIET));
        rig.until_playing(1);
        let status = rig.player.status();
        assert!(status.bit_perfect);
        assert_eq!(status.gain_db, None);

        rig.until_stopped();
        assert_played(&rig.heard(), &decode("sine.flac"), 0, 2_000);
    }

    #[test]
    fn the_gain_comes_and_goes_with_bit_perfect_mode_on_the_same_track() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play_with(1, fixture("sine.flac"), Some(QUIET));
        rig.until_playing(1);
        assert_eq!(rig.player.status().gain_db, Some(-6.0));

        rig.player.set_bit_perfect(true);
        rig.await_status("bit-perfect", |s| s.bit_perfect);
        assert_eq!(rig.player.status().gain_db, None);

        rig.player.set_bit_perfect(false);
        rig.await_status("normal output", |s| {
            !s.bit_perfect && s.state == PlaybackState::Playing
        });
        assert_eq!(rig.player.status().gain_db, Some(-6.0));
    }
}

mod equalizer {
    use super::*;
    use crate::Equalizer;

    /// A bell over the 440 Hz test tone, with the preamp pulling the whole thing down.
    const SHAPED: Equalizer = Equalizer {
        preamp_db: -3.0,
        bands: [0.0, 0.0, 0.0, 0.0, 6.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    };

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
    }

    #[test]
    fn the_device_hears_the_response_the_filters_were_designed_with_and_the_status_says_so() {
        let mut rig = rig(Config::default());
        rig.player.set_equalizer(Some(SHAPED));
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        assert!(rig.player.status().eq_active);
        rig.until_stopped();

        let wanted = SHAPED.response_db(440.0, 44_100.0);
        assert!(
            wanted.abs() > 1.0,
            "the setting should be audible: {wanted} dB"
        );
        let measured = 20.0 * (peak(&rig.heard()) / peak(&decode("sine.flac"))).log10();
        assert!(
            (f64::from(measured) - wanted).abs() < 0.25,
            "{measured} vs {wanted}"
        );
        assert!(
            !rig.player.status().eq_active,
            "nothing is playing any more"
        );
    }

    #[test]
    fn a_bit_perfect_stream_is_not_equalized_and_does_not_claim_to_be() {
        let mut rig = rig(Config::default());
        rig.player.set_equalizer(Some(SHAPED));
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        let status = rig.player.status();
        assert!(status.bit_perfect && !status.eq_active);

        rig.until_stopped();
        assert_played(&rig.heard(), &decode("sine.flac"), 0, 2_000);
    }

    #[test]
    fn eq_active_follows_the_setting_and_bit_perfect_mode_on_a_playing_track() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        assert!(!rig.player.status().eq_active);

        rig.player.set_equalizer(Some(SHAPED));
        assert!(rig.player.status().eq_active);
        rig.player.set_bit_perfect(true);
        rig.await_status("bit-perfect", |s| s.bit_perfect);
        assert!(!rig.player.status().eq_active, "remembered but not applied");

        rig.player.set_bit_perfect(false);
        rig.await_status("normal output", |s| {
            !s.bit_perfect && s.state == PlaybackState::Playing
        });
        assert!(
            rig.player.status().eq_active,
            "the setting came back with the stream"
        );
        rig.player.set_equalizer(None);
        assert!(!rig.player.status().eq_active);
    }
}

mod crossfade {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use super::*;
    use crate::TrackOptions;
    use crate::crossfade::crossfaded;

    const FADE: Duration = Duration::from_secs(1);
    /// `FADE` in frames, for tracks long enough to give all of it.
    const OVERLAP: usize = 44_100;

    /// A stereo 16-bit WAV of a sine, long enough that its end is not decoded the moment it
    /// starts: a fade can only begin if the next track is queued before that.
    struct Tone(PathBuf);

    impl Tone {
        fn new(name: &str, seconds: usize, hz: f64) -> Tone {
            let path = std::env::temp_dir().join(format!(
                "antistreaming-crossfade-{}-{name}.wav",
                std::process::id()
            ));
            let frames = 44_100 * seconds;
            let data_len = (frames * 4) as u32;
            let mut wav = Vec::new();
            wav.extend(b"RIFF");
            wav.extend((36 + data_len).to_le_bytes());
            wav.extend(b"WAVEfmt ");
            wav.extend(16u32.to_le_bytes());
            wav.extend([1, 0, 2, 0]);
            wav.extend(44_100u32.to_le_bytes());
            wav.extend((44_100u32 * 4).to_le_bytes());
            wav.extend([4, 0, 16, 0]);
            wav.extend(b"data");
            wav.extend(data_len.to_le_bytes());
            for n in 0..frames {
                let phase = 2.0 * std::f64::consts::PI * hz * n as f64 / 44_100.0;
                let sample = ((0.5 * phase.sin()) * 32_768.0) as i16;
                wav.extend(sample.to_le_bytes());
                wav.extend(sample.to_le_bytes());
            }
            fs::write(&path, wav).unwrap();
            Tone(path)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }

        fn decoded(&self) -> Vec<f32> {
            decode_path(&self.0)
        }
    }

    impl Drop for Tone {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn decode_path(path: &Path) -> Vec<f32> {
        let mut decoder = Decoder::open(path).unwrap();
        let mut samples = Vec::new();
        while let Some(chunk) = decoder.next_chunk().unwrap() {
            samples.extend_from_slice(chunk);
        }
        samples
    }

    fn album(key: &str) -> TrackOptions {
        TrackOptions {
            gain: None,
            album_key: Some(key.to_owned()),
        }
    }

    /// `(kind, track)` for every start and end event seen.
    fn starts_and_ends(rig: &Rig) -> Vec<(&'static str, i64)> {
        rig.seen
            .iter()
            .filter_map(|event| match event {
                PlayerEvent::TrackStarted { track_id } => Some(("started", *track_id)),
                PlayerEvent::TrackEnded { track_id } => Some(("ended", *track_id)),
                _ => None,
            })
            .collect()
    }

    /// Plays `first` then `second` with a crossfade wanted and returns what the device got.
    fn play_pair(
        rig: &mut Rig,
        first: (&Tone, TrackOptions),
        second: (&Tone, TrackOptions),
    ) -> Vec<f32> {
        rig.player.set_crossfade(Some(FADE));
        rig.player.play_with(1, first.0.path(), first.1);
        rig.player.queue_next_with(2, second.0.path(), second.1);
        rig.until_playing(1);
        rig.until_stopped();
        rig.heard()
    }

    #[test]
    fn the_device_hears_the_tracks_overlapped_and_each_event_comes_once_midway() {
        let (a, b) = (Tone::new("a1", 6, 440.0), Tone::new("b1", 6, 660.0));
        let mut rig = rig(Config::default());
        let heard = play_pair(
            &mut rig,
            (&a, TrackOptions::default()),
            (&b, TrackOptions::default()),
        );

        let expected = crossfaded(&a.decoded(), &b.decoded(), OVERLAP);
        assert_played(&heard, &expected, 0, 2_000);
        assert_eq!(rig.opened().len(), 1, "one stream throughout");
        assert_eq!(
            starts_and_ends(&rig),
            [("started", 1), ("ended", 1), ("started", 2), ("ended", 2)]
        );
    }

    #[test]
    fn the_status_switches_to_the_next_track_in_the_middle_of_the_overlap_at_its_own_position() {
        let (a, b) = (Tone::new("a2", 6, 440.0), Tone::new("b2", 6, 660.0));
        let mut rig = rig(Config {
            speed: 10,
            ..Config::default()
        });
        rig.player.set_crossfade(Some(FADE));
        rig.player.play(1, a.path());
        rig.player.queue_next(2, b.path());

        rig.await_status("the first track to reach its fade", |s| {
            s.track_id == Some(1) && s.position_ms >= 4_500
        });
        rig.await_status("the next track", |s| s.track_id == Some(2));
        let status = rig.player.status();
        // The overlap is 1 s and the switch is half a second into it, where the new track is
        // half a second old; the time since the switch is however long the poll took to notice.
        assert!((480..900).contains(&status.position_ms), "{status:?}");
        assert_eq!(status.duration_ms, 6_000);
        rig.until_stopped();
    }

    #[test]
    fn tracks_of_one_album_are_not_crossfaded() {
        let (a, b) = (Tone::new("a3", 6, 440.0), Tone::new("b3", 6, 660.0));
        let mut rig = rig(Config::default());
        let heard = play_pair(
            &mut rig,
            (&a, album("Kind of Blue")),
            (&b, album("Kind of Blue")),
        );

        let both = [a.decoded(), b.decoded()].concat();
        assert_played(&heard, &both, 0, 2_000);
    }

    #[test]
    fn tracks_of_different_albums_are_crossfaded() {
        let (a, b) = (Tone::new("a4", 6, 440.0), Tone::new("b4", 6, 660.0));
        let mut rig = rig(Config::default());
        let heard = play_pair(
            &mut rig,
            (&a, album("Kind of Blue")),
            (&b, album("Blue Train")),
        );

        assert_played(
            &heard,
            &crossfaded(&a.decoded(), &b.decoded(), OVERLAP),
            0,
            2_000,
        );
    }

    #[test]
    fn nothing_is_crossfaded_in_bit_perfect_mode() {
        let (a, b) = (Tone::new("a5", 6, 440.0), Tone::new("b5", 6, 660.0));
        let mut rig = rig(Config::default());
        rig.player.set_bit_perfect(true);
        let heard = play_pair(
            &mut rig,
            (&a, TrackOptions::default()),
            (&b, TrackOptions::default()),
        );

        assert!(rig.opened().iter().all(|&(_, exact)| exact));
        assert_played(&heard, &[a.decoded(), b.decoded()].concat(), 0, 2_000);
    }

    #[test]
    fn turning_the_crossfade_off_goes_back_to_plain_gapless_joins() {
        let (a, b) = (Tone::new("a6", 6, 440.0), Tone::new("b6", 6, 660.0));
        let mut rig = rig(Config::default());
        rig.player.set_crossfade(Some(FADE));
        rig.player.set_crossfade(None);
        rig.player.play(1, a.path());
        rig.player.queue_next(2, b.path());
        rig.until_stopped();

        assert_played(&rig.heard(), &[a.decoded(), b.decoded()].concat(), 0, 2_000);
    }

    #[test]
    fn a_track_queued_too_late_for_the_fade_still_follows_gaplessly() {
        // The one-second fixtures are fully decoded the moment they start, before the second
        // can be queued, so there is nothing left to fade out. At real time the second is still
        // playing when the queueing arrives, however busy the machine; a faster device could
        // finish it first.
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_crossfade(Some(FADE));
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.player.queue_next(2, fixture("sine.mp3"));
        rig.until_stopped();

        let both = [decode("sine.flac"), decode("sine.mp3")].concat();
        assert_played(&rig.heard(), &both, 0, 2_000);
    }

    #[test]
    fn playing_another_track_with_a_next_one_waiting_cuts_over_and_never_blends_it_in() {
        let (a, b) = (Tone::new("a7", 6, 440.0), Tone::new("b7", 6, 660.0));
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_crossfade(Some(FADE));
        rig.player.play(1, a.path());
        rig.player.queue_next(2, b.path());
        rig.until_playing(1);

        rig.player.play(3, fixture("sine.ogg"));
        rig.until("the new track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 3 })
        });
        rig.until_stopped();

        assert_eq!(
            starts_and_ends(&rig),
            [("started", 1), ("started", 3), ("ended", 3)],
            "the queued track never began and the replaced one never 'ended'"
        );
    }

    /// Whatever happens to a seek that lands before or after the switch, every track starts
    /// once, ends once and after it started, and nothing fails.
    fn assert_orderly(rig: &Rig) {
        let events = starts_and_ends(rig);
        for id in [1, 2] {
            let position = |kind| events.iter().position(|&event| event == (kind, id));
            assert!(position("started") < position("ended"), "{id}: {events:?}");
            let count = |kind| events.iter().filter(|&&event| event == (kind, id)).count();
            assert_eq!(
                (count("started"), count("ended")),
                (1, 1),
                "{id}: {events:?}"
            );
        }
        assert!(
            !rig.seen
                .iter()
                .any(|event| matches!(event, PlayerEvent::Error { .. })),
            "{:?}",
            rig.seen
        );
    }

    #[test]
    fn seeking_in_the_first_half_of_the_overlap_returns_to_the_current_track_and_fades_again() {
        let (a, b) = (Tone::new("a8", 6, 440.0), Tone::new("b8", 6, 660.0));
        let mut rig = rig(Config {
            speed: 4,
            ..Config::default()
        });
        rig.player.set_crossfade(Some(FADE));
        rig.player.play(1, a.path());
        rig.player.queue_next(2, b.path());
        // The overlap is the last second of the first track; its first half is before the switch.
        rig.await_status("the overlap to begin", |s| {
            s.track_id == Some(1) && s.position_ms >= 5_100
        });

        rig.player.seek(2_000);
        rig.await_status("the first track again", |s| {
            s.track_id == Some(1) && (2_000..4_000).contains(&s.position_ms)
        });
        rig.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        rig.until_stopped();
        assert_orderly(&rig);
    }

    #[test]
    fn seeking_in_the_second_half_of_the_overlap_carries_on_in_the_next_track() {
        let (a, b) = (Tone::new("a9", 6, 440.0), Tone::new("b9", 6, 660.0));
        let mut rig = rig(Config {
            speed: 4,
            ..Config::default()
        });
        rig.player.set_crossfade(Some(FADE));
        rig.player.play(1, a.path());
        rig.player.queue_next(2, b.path());
        rig.await_status("the switch", |s| s.track_id == Some(2));

        rig.player.seek(3_000);
        rig.await_status("the second track, further on", |s| {
            s.track_id == Some(2) && (3_000..5_500).contains(&s.position_ms)
        });
        rig.until("the second track to end", |e| {
            matches!(e, PlayerEvent::TrackEnded { track_id: 2 })
        });
        rig.until_stopped();
        assert_orderly(&rig);
        assert!(
            rig.player.status().position_ms == 0,
            "playback ended: {:?}",
            rig.player.status()
        );
    }
}

mod live {
    use std::io::{self, Read};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::{LiveFormat, TrackOptions};

    fn bytes(name: &str) -> Vec<u8> {
        std::fs::read(fixture(name)).unwrap()
    }

    /// How a `Trickle` ends once its bytes are used up.
    #[derive(Clone, Copy)]
    enum Then {
        Eof,
        Fail(io::ErrorKind),
    }

    /// A connection: bytes in small pieces, with stalls at chosen offsets, ending as told.
    struct Trickle {
        data: Vec<u8>,
        at: usize,
        piece: usize,
        stalls: Vec<(usize, Duration)>,
        then: Then,
        dropped: Arc<AtomicBool>,
    }

    impl Trickle {
        fn new(data: Vec<u8>) -> Trickle {
            Trickle {
                data,
                at: 0,
                piece: 1_024,
                stalls: Vec::new(),
                then: Then::Eof,
                dropped: Arc::new(AtomicBool::new(false)),
            }
        }

        fn stalling(mut self, at: usize, for_how_long: Duration) -> Trickle {
            self.stalls.push((at, for_how_long));
            self
        }

        fn ending(mut self, then: Then) -> Trickle {
            self.then = then;
            self
        }

        fn watched(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.dropped)
        }
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(stall) = self.stalls.iter().position(|&(at, _)| at <= self.at) {
                std::thread::sleep(self.stalls.remove(stall).1);
            }
            if self.at >= self.data.len() {
                return match self.then {
                    Then::Eof => Ok(0),
                    Then::Fail(kind) => Err(io::Error::new(kind, "the connection dropped")),
                };
            }
            let count = self.piece.min(buf.len()).min(self.data.len() - self.at);
            buf[..count].copy_from_slice(&self.data[self.at..self.at + count]);
            self.at += count;
            Ok(count)
        }
    }

    impl Drop for Trickle {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    fn play(rig: &Rig, source: impl Read + Send + 'static, hint: LiveFormat) {
        rig.player
            .play_live(1, Box::new(source), hint, TrackOptions::default());
    }

    impl Rig {
        fn until_live_ended(&mut self) -> String {
            match self.until("the live stream to end", |e| {
                matches!(e, PlayerEvent::LiveEnded { .. })
            }) {
                PlayerEvent::LiveEnded { reason, .. } => reason,
                _ => unreachable!(),
            }
        }
    }

    /// What FFmpeg makes of a fixture read as a file, for the formats live streams hand to it.
    #[cfg(feature = "ffmpeg")]
    fn decode_with_ffmpeg(name: &str) -> Vec<f32> {
        use crate::decoder::Source;
        let mut source = crate::ffmpeg_source::FfmpegSource::open(&fixture(name)).unwrap();
        let mut samples = Vec::new();
        while let Some(chunk) = source.read_chunk().unwrap() {
            samples.extend_from_slice(chunk);
        }
        samples
    }

    /// Every format a live stream can be in, with the file it is played from.
    fn formats() -> Vec<(&'static str, LiveFormat)> {
        let always = [
            ("sine.mp3", LiveFormat::Mp3),
            ("live-adts.aac", LiveFormat::AacAdts),
            ("sine.ogg", LiveFormat::Ogg),
            ("sine.flac", LiveFormat::Flac),
        ];
        // The rest are read by FFmpeg alone.
        #[cfg(feature = "ffmpeg")]
        let ffmpeg_only = [
            ("sine.opus", LiveFormat::Opus),
            ("live-seg0.ts", LiveFormat::MpegTs),
            ("live-fragmented.mp4", LiveFormat::Fmp4),
        ];
        #[cfg(not(feature = "ffmpeg"))]
        let ffmpeg_only = [];
        always.into_iter().chain(ffmpeg_only).collect()
    }

    /// What the same file decodes to when read as a file.
    fn as_a_file(file: &str, hint: LiveFormat) -> Vec<f32> {
        match hint {
            // symphonia would read fragmented mp4 as a file, and mistake transport streams
            // for something else, but live streams of them go to FFmpeg.
            #[cfg(feature = "ffmpeg")]
            LiveFormat::Fmp4 | LiveFormat::MpegTs => decode_with_ffmpeg(file),
            _ => decode(file),
        }
    }

    /// Plays `file` as a live stream hinted as `hint` and checks the device got what a file
    /// of it gives.
    fn assert_plays_as_a_file(file: &str, hint: LiveFormat, told_as: LiveFormat) {
        let mut rig = rig(Config::default());
        play(&rig, Trickle::new(bytes(file)), told_as);
        rig.until_live_ended();
        rig.until_stopped();
        assert_played(&rig.heard(), &as_a_file(file, hint), 0, 2_000);
        assert!(
            !rig.seen.iter().any(|event| matches!(
                event,
                PlayerEvent::TrackStarted { .. } | PlayerEvent::TrackEnded { .. }
            )),
            "a station is not a play: {file} {:?}",
            rig.seen
        );
    }

    #[test]
    fn every_format_plays_as_it_would_from_a_file() {
        for (file, hint) in formats() {
            assert_plays_as_a_file(file, hint, hint);
        }
    }

    #[test]
    fn a_stream_of_unknown_type_is_recognised_from_its_first_bytes() {
        for (file, hint) in formats() {
            assert_plays_as_a_file(file, hint, LiveFormat::Unknown);
        }
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn a_wrong_hint_gives_way_to_what_the_bytes_plainly_are() {
        // Transport streams and Ogg are unmistakable; and what symphonia cannot read falls back.
        assert_plays_as_a_file("live-seg0.ts", LiveFormat::MpegTs, LiveFormat::Mp3);
        assert_plays_as_a_file("sine.opus", LiveFormat::Opus, LiveFormat::Ogg);
        assert_plays_as_a_file("sine.ogg", LiveFormat::Ogg, LiveFormat::Opus);
        assert_plays_as_a_file("live-fragmented.mp4", LiveFormat::Fmp4, LiveFormat::AacAdts);
    }

    /// A file removed when dropped.
    #[cfg(feature = "ffmpeg")]
    struct Scratch(PathBuf);

    #[cfg(feature = "ffmpeg")]
    impl Scratch {
        fn new(name: &str, contents: &[u8]) -> Scratch {
            let path = std::env::temp_dir()
                .join(format!("antistreaming-live-{}-{name}", std::process::id()));
            std::fs::write(&path, contents).unwrap();
            Scratch(path)
        }
    }

    #[cfg(feature = "ffmpeg")]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn transport_stream_segments_one_after_another_play_as_one_stream() {
        let both = [bytes("live-seg0.ts"), bytes("live-seg1.ts")].concat();
        // Decoded as one stream, the second segment carries on from the first, which is not what
        // decoding each alone gives; the whole thing as a file is the same thing.
        let joined = Scratch::new("joined.ts", &both);
        let expected = {
            use crate::decoder::Source;
            let mut source = crate::ffmpeg_source::FfmpegSource::open(&joined.0).unwrap();
            let mut samples = Vec::new();
            while let Some(chunk) = source.read_chunk().unwrap() {
                samples.extend_from_slice(chunk);
            }
            samples
        };

        let mut rig = rig(Config::default());
        play(&rig, Trickle::new(both), LiveFormat::MpegTs);
        rig.until_live_ended();
        rig.until_stopped();

        assert!(
            expected.len() > 2 * 44_100 * 19 / 10,
            "two segments of audio"
        );
        assert_played(&rig.heard(), &expected, 0, 2_000);
    }

    #[test]
    fn a_stream_joined_in_the_middle_of_a_frame_starts_at_the_next_one() {
        let mid = bytes("sine.mp3")[100..].to_vec();
        let mut rig = rig(Config::default());
        play(&rig, Trickle::new(mid), LiveFormat::Mp3);
        rig.until_live_ended();
        rig.until_stopped();

        let heard = rig.heard();
        let peak = heard.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        let sounding = heard.iter().filter(|s| s.abs() > 0.01).count();
        assert!((0.45..0.55).contains(&peak), "{peak}");
        assert!(
            sounding > 2 * 40_000,
            "nearly all of the second is heard: {sounding}"
        );
        assert!(
            !rig.seen
                .iter()
                .any(|e| matches!(e, PlayerEvent::Error { .. }))
        );
    }

    /// What the events say about buffering: `buffering` each time it is reported, `playing` each
    /// time it is over, and `ended` for the end of the stream.
    fn buffering_story(rig: &Rig) -> Vec<&'static str> {
        let mut story = Vec::new();
        for event in &rig.seen {
            let word = match event {
                PlayerEvent::Buffering { .. } => "buffering",
                PlayerEvent::StateChanged(status) if status.live && !status.buffering => "playing",
                PlayerEvent::LiveEnded { .. } => "ended",
                _ => continue,
            };
            if story.last() != Some(&word) {
                story.push(word);
            }
        }
        story
    }

    /// The sound in `heard`, with silences of a few hundred frames or more taken out.
    fn without_silences(heard: &[f32]) -> Vec<f32> {
        let mut sound = Vec::new();
        let mut quiet = 0;
        for &sample in heard {
            quiet = if sample == 0.0 { quiet + 1 } else { 0 };
            sound.push(sample);
            if quiet == 600 {
                sound.truncate(sound.len() - 600);
            } else if quiet > 600 {
                sound.pop();
            }
        }
        sound
    }

    #[test]
    fn running_dry_holds_playback_and_reports_buffering_until_there_is_enough_again() {
        let mut rig = rig(Config {
            speed: 10,
            ..Config::default()
        });
        rig.player.set_live_buffer(Duration::from_millis(250));
        // About 0.4 s of audio, then nothing for most of a second, then the rest.
        let stream = Trickle::new(bytes("sine.mp3")).stalling(3_200, Duration::from_millis(900));
        play(&rig, stream, LiveFormat::Mp3);
        rig.until_live_ended();
        rig.until_stopped();

        assert_eq!(
            buffering_story(&rig),
            ["buffering", "playing", "buffering", "playing", "ended"]
        );
        // Nothing was skipped or repeated: the audio on both sides of the gap is the file's.
        let expected = decode("sine.mp3");
        let sound = without_silences(&rig.heard());
        for from in [10_000, expected.len() - 20_000] {
            let at = find(&sound, &expected[from..from + 256]).expect("that stretch was played");
            assert!(
                sound[at..at + 4_000] == expected[from..from + 4_000],
                "from {from}"
            );
        }
    }

    #[test]
    fn nothing_plays_until_two_seconds_are_decoded_unless_the_stream_ends_first() {
        let mut rig = rig(Config::default());
        // A second of audio, which is less than the two that are waited for, and then a quiet
        // spell before the stream ends.
        let data = bytes("sine.mp3");
        let end = data.len();
        play(
            &rig,
            Trickle::new(data).stalling(end, Duration::from_millis(1_200)),
            LiveFormat::Mp3,
        );
        rig.until("buffering", |e| matches!(e, PlayerEvent::Buffering { .. }));

        std::thread::sleep(Duration::from_millis(600));
        let status = rig.player.status();
        assert!(status.live && status.buffering, "{status:?}");
        assert_eq!(status.position_ms, 0);
        assert!(
            rig.heard().iter().all(|&sample| sample == 0.0),
            "silent so far"
        );

        rig.until_live_ended();
        assert!(
            rig.heard().iter().any(|&sample| sample != 0.0),
            "what arrived was played"
        );
    }

    #[test]
    fn a_change_of_sample_rate_partway_through_is_followed_by_resampling() {
        let ogg = (
            [bytes("sine.ogg"), bytes("live-22k.ogg")].concat(),
            LiveFormat::Ogg,
        );
        #[cfg(feature = "ffmpeg")]
        let streams = [
            ogg,
            (
                [bytes("live-seg0.ts"), bytes("live-48k.ts")].concat(),
                LiveFormat::MpegTs,
            ),
        ];
        #[cfg(not(feature = "ffmpeg"))]
        let streams = [ogg];
        for (data, hint) in streams {
            let mut rig = rig(Config::default());
            play(&rig, Trickle::new(data), hint);
            rig.until_live_ended();
            rig.until_stopped();

            assert_eq!(rig.opened(), [(44_100, false)], "one stream throughout");
            let heard = rig.heard();
            let left: Vec<f32> = heard.iter().step_by(2).copied().collect();
            let start = left.iter().position(|&s| s != 0.0).unwrap();
            let sounding = left.iter().filter(|s| s.abs() > 0.01).count();
            assert!(
                (84_000..90_000).contains(&sounding),
                "two seconds: {sounding}"
            );
            // Over the second part, a 440 Hz tone crosses zero about 880 times a second; played
            // without resampling, 48 kHz audio on a 44.1 kHz device would make it 958.
            let window = &left[start + 55_000..start + 85_000];
            let crossings = window
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            let wanted = 880.0 * window.len() as f64 / 44_100.0;
            assert!(
                (crossings as f64 - wanted).abs() < 0.04 * wanted,
                "{crossings} vs {wanted}"
            );
            assert!(
                !rig.seen
                    .iter()
                    .any(|e| matches!(e, PlayerEvent::Error { .. }))
            );
        }
    }

    #[test]
    fn an_mp3_stream_that_changes_rate_is_given_up_on_with_a_reason_not_left_silent() {
        // symphonia's MP3 reader takes frames of another rate for junk and searches the rest of
        // the stream for ones it likes; after a while with nothing to show for it, the stream
        // ends and the caller can connect again, to a stream that starts afresh in the new rate.
        let data = [bytes("sine.mp3"), bytes("live-48k.mp3").repeat(50)].concat();
        let mut rig = rig(Config::default());
        play(&rig, Trickle::new(data), LiveFormat::Mp3);
        let reason = rig.until_live_ended();
        rig.until_stopped();

        assert!(reason.contains("stopped producing audio"), "{reason}");
        let sounding = rig.heard().iter().filter(|s| s.abs() > 0.01).count();
        assert!(
            (80_000..90_000).contains(&sounding),
            "the first second was played: {sounding}"
        );
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn the_status_follows_the_streams_sample_rate() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        let both = [bytes("live-seg0.ts"), bytes("live-48k.ts")].concat();
        play(&rig, Trickle::new(both), LiveFormat::MpegTs);
        rig.await_status("the first part", |s| {
            s.live && s.sample_rate == Some(44_100)
        });
        rig.await_status("the second part", |s| {
            s.live && s.sample_rate == Some(48_000)
        });
        assert_eq!(rig.player.status().output_sample_rate, Some(44_100));
        rig.until_live_ended();
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn a_bit_perfect_stream_reopens_the_device_for_a_change_of_rate() {
        let mut rig = rig(Config::default());
        rig.player.set_bit_perfect(true);
        let both = [bytes("live-seg0.ts"), bytes("live-48k.ts")].concat();
        play(&rig, Trickle::new(both), LiveFormat::MpegTs);
        rig.until_live_ended();
        rig.until_stopped();

        assert_eq!(rig.opened(), [(44_100, true), (48_000, true)]);
        assert!(rig.notices().is_empty(), "{:?}", rig.notices());
        let sounding = rig.heard().iter().filter(|s| s.abs() > 0.01).count();
        assert!(
            sounding > 2 * 2 * 40_000,
            "both parts were played: {sounding}"
        );
    }

    #[test]
    fn a_lossless_stream_switches_a_bit_perfect_device_to_its_rate() {
        let mut rig = rig(Config::default());
        rig.player.set_bit_perfect(true);
        play(
            &rig,
            Trickle::new(bytes("sine-24bit-96k.flac")),
            LiveFormat::Flac,
        );
        rig.await_status("bit-perfect", |s| s.live && s.bit_perfect);
        let status = rig.player.status();
        assert_eq!(status.output_sample_rate, Some(96_000));
        assert_eq!(status.volume, 1.0);
        rig.until_live_ended();
        rig.until_stopped();

        assert_eq!(rig.opened(), [(96_000, true)]);
        assert_played(&rig.heard(), &decode("sine-24bit-96k.flac"), 0, 2_000);
    }

    #[test]
    fn switching_bit_perfect_mode_moves_a_live_stream_to_a_new_output_and_buffers_again() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        let data = bytes("sine.mp3");
        let end = data.len();
        rig.player.set_live_buffer(Duration::from_millis(250));
        play(
            &rig,
            Trickle::new(data).stalling(end, Duration::from_secs(2)),
            LiveFormat::Mp3,
        );
        rig.await_status("playing", |s| {
            s.live && !s.buffering && s.position_ms >= 200
        });

        rig.player.set_bit_perfect(true);
        rig.await_status("bit-perfect", |s| s.live && s.bit_perfect);
        let status = rig.player.status();
        assert!(
            status.buffering,
            "the stream was moved and starts afresh: {status:?}"
        );
        assert_eq!(rig.opened(), [(44_100, false), (44_100, true)]);
        // The time already heard is not forgotten.
        assert!(status.position_ms >= 200, "{status:?}");
        let story = rig.settle().len();
        assert!(story > 0);
    }

    #[test]
    fn a_dropped_connection_ends_the_stream_once_what_arrived_has_played() {
        #[cfg(feature = "ffmpeg")]
        let cases = [
            ("sine.mp3", LiveFormat::Mp3),
            ("live-seg0.ts", LiveFormat::MpegTs),
        ];
        #[cfg(not(feature = "ffmpeg"))]
        let cases = [("sine.mp3", LiveFormat::Mp3)];
        for (file, hint) in cases {
            let data = bytes(file)[..6_000].to_vec();
            let mut rig = rig(Config::default());
            play(
                &rig,
                Trickle::new(data).ending(Then::Fail(io::ErrorKind::ConnectionReset)),
                hint,
            );
            let reason = rig.until_live_ended();
            rig.until_stopped();

            assert!(!reason.is_empty(), "{file}");
            let peak = rig
                .heard()
                .iter()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()));
            assert!(peak > 0.3, "{file}: what had arrived was played ({peak})");
            let status = rig.player.status();
            assert!(
                !status.live && status.state == PlaybackState::Stopped,
                "{status:?}"
            );
        }
    }

    #[test]
    fn the_reason_a_stream_ended_is_given() {
        let mut dropped = rig(Config::default());
        let data = bytes("sine.mp3")[..4_000].to_vec();
        play(
            &dropped,
            Trickle::new(data).ending(Then::Fail(io::ErrorKind::TimedOut)),
            LiveFormat::Mp3,
        );
        assert!(
            dropped
                .until_live_ended()
                .contains("the connection dropped")
        );

        let mut finished = rig(Config::default());
        play(&finished, Trickle::new(bytes("sine.mp3")), LiveFormat::Mp3);
        assert_eq!(finished.until_live_ended(), "The stream ended.");
    }

    #[test]
    fn streams_that_are_not_audio_or_not_there_end_with_a_reason_instead_of_hanging() {
        let nothing_at_all = Trickle::new(Vec::new());
        let refused = Trickle::new(Vec::new()).ending(Then::Fail(io::ErrorKind::ConnectionRefused));
        let noise = Trickle::new(bytes("not-audio.txt").repeat(400));
        let streams: [Box<dyn Read + Send>; 3] =
            [Box::new(nothing_at_all), Box::new(refused), Box::new(noise)];
        for source in streams {
            let mut rig = rig(Config::default());
            rig.player
                .play_live(1, source, LiveFormat::Mp3, TrackOptions::default());
            let reason = rig.until_live_ended();
            rig.until_stopped();
            assert!(!reason.is_empty());
            assert!(!rig.player.status().live);
            assert!(rig.heard().iter().all(|&s| s == 0.0), "nothing to hear");
        }
    }

    #[test]
    fn a_reader_that_panics_is_a_stream_that_ended_not_a_dead_engine() {
        struct Panics;
        impl Read for Panics {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("the network library fell over");
            }
        }
        let mut rig = rig(Config::default());
        play(&rig, Panics, LiveFormat::Mp3);
        assert!(rig.until_live_ended().contains("crashed"));
        // The engine carries on.
        rig.player.play(2, fixture("sine.flac"));
        rig.until_playing(2);
    }

    /// A stream that goes on for as long as the test needs: a second of audio, then a long wait.
    fn endless() -> Trickle {
        let data = bytes("sine.mp3");
        let end = data.len();
        Trickle::new(data).stalling(end, Duration::from_millis(1_500))
    }

    #[test]
    fn seeking_a_live_stream_does_nothing_and_the_time_counts_up() {
        let rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_live_buffer(Duration::from_millis(250));
        play(&rig, endless(), LiveFormat::Mp3);
        rig.await_status("some of it played", |s| s.live && s.position_ms >= 300);

        rig.player.seek(60_000);
        std::thread::sleep(Duration::from_millis(300));
        let status = rig.player.status();
        assert!(status.live && status.duration_ms == 0, "{status:?}");
        assert!(
            (300..1_100).contains(&status.position_ms),
            "still where it was, counting up: {status:?}"
        );
        assert!(
            !rig.seen
                .iter()
                .any(|e| matches!(e, PlayerEvent::Error { .. }))
        );
    }

    #[test]
    fn pausing_a_live_stream_stops_it_and_lets_go_of_the_connection() {
        let rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_live_buffer(Duration::from_millis(250));
        let stream = endless();
        let dropped = stream.watched();
        play(&rig, stream, LiveFormat::Mp3);
        rig.await_status("playing", |s| {
            s.live && !s.buffering && s.position_ms >= 100
        });

        rig.player.pause();
        rig.await_status("stopped", |s| s.state == PlaybackState::Stopped);
        let status = rig.player.status();
        assert!(!status.live && status.track_id.is_none(), "{status:?}");

        // Nothing to resume: it stays stopped.
        rig.player.resume();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(rig.player.status().state, PlaybackState::Stopped);

        // The connection is released as soon as the reader lets the decoder go.
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dropped.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the stream was never let go");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_stream_still_connecting_shows_as_buffering_and_can_be_stopped_or_replaced() {
        let rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        // The first byte is a long time coming.
        let slow = Trickle::new(bytes("sine.mp3")).stalling(0, Duration::from_millis(1_500));
        let dropped = slow.watched();
        play(&rig, slow, LiveFormat::Mp3);
        rig.await_status("connecting", |s| s.track_id == Some(1));
        let status = rig.player.status();
        assert!(status.live && status.buffering, "{status:?}");
        assert_eq!(status.state, PlaybackState::Playing);
        assert_eq!(status.sample_rate, None);

        rig.player.pause();
        rig.await_status("stopped", |s| s.state == PlaybackState::Stopped);
        assert!(!rig.player.status().live);

        // Replacing one that is still connecting lets the first go too.
        rig.player.play(2, fixture("sine.flac"));
        rig.await_status("the track", |s| s.track_id == Some(2));
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dropped.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the stream was never let go");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn nothing_is_queued_behind_a_live_stream() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_live_buffer(Duration::from_millis(250));
        play(&rig, endless(), LiveFormat::Mp3);
        rig.player.queue_next(2, fixture("sine.flac"));
        rig.until_live_ended();
        rig.until_stopped();
        rig.settle();

        assert!(
            !rig.seen
                .iter()
                .any(|e| matches!(e, PlayerEvent::TrackStarted { .. })),
            "the queued track never began: {:?}",
            rig.seen
        );
    }

    #[test]
    fn playing_a_track_replaces_a_live_stream_and_the_other_way_round() {
        let mut rig = rig(Config {
            speed: 1,
            ..Config::default()
        });
        rig.player.set_live_buffer(Duration::from_millis(250));
        let stream = endless();
        let dropped = stream.watched();
        play(&rig, stream, LiveFormat::Mp3);
        rig.await_status("live", |s| s.live && !s.buffering);

        rig.player.play(2, fixture("sine.flac"));
        rig.until_playing(2);
        let status = rig.player.status();
        assert!(!status.live && status.duration_ms == 1_000, "{status:?}");
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dropped.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the stream was never let go");
            std::thread::sleep(Duration::from_millis(20));
        }

        // And a station can follow a track, which is not announced as having ended.
        rig.player.play_live(
            3,
            Box::new(endless()),
            LiveFormat::Mp3,
            TrackOptions::default(),
        );
        rig.await_status("live again", |s| s.live && s.track_id == Some(3));
        assert!(
            !rig.seen
                .iter()
                .any(|e| matches!(e, PlayerEvent::TrackEnded { track_id: 2 }))
        );
    }

    #[test]
    fn sound_check_does_not_touch_a_live_stream() {
        let mut rig = rig(Config::default());
        let loud = crate::Gain {
            db: -12.0,
            peak_dbtp: -6.0,
        };
        rig.player.play_live(
            1,
            Box::new(Trickle::new(bytes("sine.flac"))),
            LiveFormat::Flac,
            loud,
        );
        rig.until_live_ended();
        rig.until_stopped();
        assert_played(&rig.heard(), &decode("sine.flac"), 0, 2_000);
        assert!(rig.seen.iter().all(|e| !matches!(e,
            PlayerEvent::StateChanged(s) if s.gain_db.is_some())));
    }

    #[cfg(not(feature = "ffmpeg"))]
    #[test]
    fn a_stream_only_ffmpeg_reads_ends_with_a_reason_when_it_is_left_out() {
        let mut rig = rig(Config::default());
        play(&rig, Trickle::new(bytes("sine.opus")), LiveFormat::Opus);
        assert!(rig.until_live_ended().contains("ffmpeg"));
    }
}

mod tap {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread::{self, JoinHandle};

    use super::*;
    use crate::Tap;

    /// Everything a tap handed over while a track played.
    struct Tapped {
        samples: Vec<f32>,
        stamps: Vec<Duration>,
        /// The (rate, channels) of each run of blocks.
        formats: Vec<(u32, u16)>,
        dropped: u64,
        clock_at_end: Duration,
    }

    /// Reads `tap` as a visualizer would, from a thread of its own, until told to finish.
    struct Reader {
        finish: Arc<AtomicBool>,
        blocks: Arc<AtomicUsize>,
        thread: JoinHandle<Tapped>,
    }

    impl Reader {
        fn start(mut tap: Tap) -> Reader {
            let finish = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&finish);
            let blocks = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&blocks);
            let thread = thread::spawn(move || {
                let mut tapped = Tapped {
                    samples: Vec::new(),
                    stamps: Vec::new(),
                    formats: Vec::new(),
                    dropped: 0,
                    clock_at_end: Duration::ZERO,
                };
                loop {
                    let finishing = flag.load(Ordering::Acquire);
                    while let Some(block) = tap.read() {
                        let format = (block.sample_rate, block.channels);
                        if tapped.formats.last() != Some(&format) {
                            tapped.formats.push(format);
                        }
                        tapped.stamps.push(block.at);
                        count.fetch_add(1, Ordering::Release);
                        tapped.samples.extend_from_slice(block.samples);
                    }
                    if finishing {
                        tapped.dropped = tap.dropped();
                        tapped.clock_at_end = tap.now();
                        return tapped;
                    }
                    thread::sleep(Duration::from_micros(500));
                }
            });
            Reader {
                finish,
                blocks,
                thread,
            }
        }

        /// Waits until the first block has been read.
        fn started(&self) {
            let deadline = Instant::now() + PATIENCE;
            while self.blocks.load(Ordering::Acquire) == 0 {
                assert!(Instant::now() < deadline, "the tap never delivered a block");
                thread::sleep(Duration::from_millis(1));
            }
        }

        fn finish(self) -> Tapped {
            self.finish.store(true, Ordering::Release);
            self.thread.join().unwrap()
        }
    }

    /// The device plays in real time, so a reader polling every half millisecond has the tap's
    /// whole half second of slack against a busy machine, and nothing is dropped.
    fn realtime() -> Config {
        Config {
            speed: 1,
            ..Config::default()
        }
    }

    #[test]
    fn the_tap_has_what_the_device_was_given_and_when() {
        let mut rig = rig(realtime());
        let reader = Reader::start(rig.player.tap());
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.until_stopped();
        let tapped = reader.finish();

        assert_eq!(tapped.dropped, 0);
        assert_eq!(tapped.formats, [(44_100, 2)]);
        let heard = rig.heard();
        assert!(
            tapped.samples.len() > 4_256,
            "{} samples tapped",
            tapped.samples.len()
        );
        let at = find(&heard, &tapped.samples[4_000..4_256]).expect("the tap matches the device");
        let start = at - 4_000;
        assert!(
            heard[start..start + tapped.samples.len()] == tapped.samples[..],
            "at full volume the tap is the device's own samples"
        );
        assert!(tapped.stamps.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(tapped.stamps.iter().all(|&at| at <= tapped.clock_at_end));
    }

    #[test]
    fn the_tap_ignores_the_volume_that_quietens_the_device() {
        let mut rig = rig(realtime());
        rig.player.set_volume(0.3);
        let reader = Reader::start(rig.player.tap());
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.until_stopped();
        let tapped = reader.finish();

        let source = decode("sine.flac");
        assert_eq!(tapped.dropped, 0);
        assert_played(&tapped.samples, &source, 0, 2_000);
        let peak = |samples: &[f32]| samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak(&rig.heard()) < 0.05 * peak(&source),
            "the device is quiet"
        );
    }

    #[test]
    fn a_bit_perfect_stream_is_tapped_unchanged_too() {
        let mut rig = rig(realtime());
        let reader = Reader::start(rig.player.tap());
        rig.player.set_bit_perfect(true);
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.until_stopped();
        let tapped = reader.finish();

        assert_eq!(tapped.dropped, 0);
        assert_played(&tapped.samples, &decode("sine.flac"), 0, 2_000);
        assert_played(&rig.heard(), &decode("sine.flac"), 0, 2_000);
    }

    #[test]
    fn a_new_output_format_reaches_the_tap_with_its_blocks() {
        let mut rig = rig(realtime());
        let reader = Reader::start(rig.player.tap());
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        // The reopen must find the first format already read, or there would be nothing to see change.
        reader.started();
        rig.player.play(2, fixture("sine-24bit-96k.flac"));
        rig.until_playing(2);
        rig.until_stopped();
        let tapped = reader.finish();

        let rates: Vec<u32> = tapped.formats.iter().map(|&(rate, _)| rate).collect();
        assert_eq!(rates, [44_100, 96_000]);
        assert_eq!(rig.opened().len(), 2);
    }

    #[test]
    fn a_tap_nobody_reads_drops_blocks_and_playback_carries_on() {
        let mut rig = rig(Config::default());
        let tap = rig.player.tap();
        rig.player.play(1, fixture("sine.flac"));
        rig.until_playing(1);
        rig.until_stopped();

        let source = decode("sine.flac");
        let half_second = 44_100; // stereo samples at 44.1 kHz
        assert!(
            source.len() > half_second + 8_192,
            "the track outlasts what the tap keeps"
        );
        assert!(tap.dropped() > 0);
        assert_played(&rig.heard(), &source, 0, 2_000);
    }
}
