//! Bit-perfect output on Windows: a WASAPI exclusive-mode stream, which cpal cannot open.
//!
//! In exclusive mode the device is ours alone and runs at exactly the format we ask for, so the
//! engine's ring is drained by a thread of our own that feeds the same callback cpal would.
//! Every WASAPI object lives on that thread, since COM interfaces do not move between them.
//!
//! This has only been compile-checked: there is no Windows machine to run it on.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wasapi::{
    AudioClient, AudioRenderClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode,
    WaveFormat, deinitialize, initialize_mta,
};

use crate::config::{Sample, depth_label, exclusive_candidates, format_label, pack_24_bit};
use crate::host::Failure;
use crate::output::{Callback, Controls, Format, Output, OutputInfo, Pipe, Request, Stream};
use crate::{Error, Result};

/// How long to wait for the device to ask for more audio before deciding it has gone.
const EVENT_TIMEOUT_MS: u32 = 1000;

/// Opens the default render device in exclusive mode at the request's rate and channel count.
pub fn open(request: &Request, controls: Controls, on_failure: Failure) -> Result<Output> {
    let source = request.source;
    let (callback, pipe) = Pipe::new(source, controls, true);
    let control = Arc::new(Control::default());
    let (ready, opened) = mpsc::channel();

    let thread = {
        let (control, depth) = (Arc::clone(&control), request.bit_depth);
        thread::Builder::new()
            .name("antistreaming-wasapi".into())
            .spawn(move || render_thread(source, depth, callback, &control, ready, on_failure))?
    };
    let stream = WasapiStream {
        control,
        thread: Mutex::new(Some(thread)),
    };

    // The stream's own drop joins the thread if opening failed.
    let sample = match opened.recv() {
        Ok(Ok(sample)) => sample,
        Ok(Err(reason)) => return Err(Error::BitPerfectUnavailable(reason)),
        Err(_) => {
            return Err(Error::Device(
                "the render thread ended before opening".into(),
            ));
        }
    };
    let depth = if sample.float {
        depth_label(cpal::SampleFormat::F32).to_owned()
    } else {
        format!("{}-bit", sample.valid_bits)
    };
    let info = OutputInfo {
        bit_perfect: true,
        label: format_label(&depth, source.sample_rate),
    };
    Ok(pipe.into_output(Box::new(stream), source, *request, info))
}

#[derive(Default)]
struct Control {
    start: AtomicBool,
    stop: AtomicBool,
}

struct WasapiStream {
    control: Arc<Control>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Stream for WasapiStream {
    fn start(&self) -> Result<()> {
        self.control.start.store(true, Ordering::Release);
        Ok(())
    }
}

impl Drop for WasapiStream {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// COM is initialised for the thread's lifetime.
struct ComScope;

impl ComScope {
    fn new() -> ComScope {
        let _ = initialize_mta();
        ComScope
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        deinitialize();
    }
}

/// An initialised exclusive-mode stream and the buffers to feed it.
struct Session {
    client: AudioClient,
    render: AudioRenderClient,
    events: Handle,
    sample: Sample,
    channels: usize,
    block_align: usize,
    /// How long the device's whole buffer takes to play, which is how far ahead of the speakers
    /// each fill is.
    latency: Duration,
    scratch: Scratch,
    bytes: Vec<u8>,
}

/// Room for one device buffer of each sample type the callback can render.
struct Scratch {
    ints16: Vec<i16>,
    ints32: Vec<i32>,
    floats: Vec<f32>,
}

fn render_thread(
    source: Format,
    bit_depth: Option<u32>,
    mut callback: Callback,
    control: &Control,
    ready: mpsc::Sender<std::result::Result<Sample, String>>,
    on_failure: Failure,
) {
    let _com = ComScope::new();
    let mut session = match Session::open(source, bit_depth) {
        Ok(session) => session,
        Err(reason) => {
            let _ = ready.send(Err(reason));
            return;
        }
    };
    let _ = ready.send(Ok(session.sample));
    callback.set_latency(session.latency);

    // Wait for the engine to prime the ring, so the first buffers already hold audio.
    while !control.start.load(Ordering::Acquire) {
        if control.stop.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let outcome = session
        .fill(&mut callback)
        .and_then(|()| session.client.start_stream().map_err(|e| e.to_string()))
        .and_then(|()| session.run(&mut callback, control));
    if let Err(reason) = outcome {
        on_failure(Error::Device(reason));
    }
    let _ = session.client.stop_stream();
}

impl Session {
    fn open(source: Format, bit_depth: Option<u32>) -> std::result::Result<Session, String> {
        let unavailable = |what: &str| {
            format!(
                "the device has no exclusive {}-channel {} format ({what})",
                source.channels,
                format_label("", source.sample_rate).trim()
            )
        };
        let device = DeviceEnumerator::new()
            .and_then(|enumerator| enumerator.get_default_device(&Direction::Render))
            .map_err(|e| e.to_string())?;
        let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;

        let (sample, format) = exclusive_candidates(bit_depth)
            .into_iter()
            .find_map(|sample| {
                let kind = if sample.float {
                    SampleType::Float
                } else {
                    SampleType::Int
                };
                let wanted = WaveFormat::new(
                    usize::from(sample.container_bits),
                    usize::from(sample.valid_bits),
                    &kind,
                    source.sample_rate as usize,
                    usize::from(source.channels),
                    None,
                );
                let accepted = client.is_supported_exclusive_with_quirks(&wanted).ok()?;
                Some((sample, accepted))
            })
            .ok_or_else(|| unavailable("no sample layout was accepted"))?;

        let (_, minimum) = client.get_device_period().map_err(|e| e.to_string())?;
        // 128-byte alignment satisfies Intel HDA devices, which reject anything else.
        let period = client
            .calculate_aligned_period_near(3 * minimum / 2, Some(128), &format)
            .map_err(|e| e.to_string())?;
        client
            .initialize_client(
                &format,
                &Direction::Render,
                &StreamMode::EventsExclusive { period_hns: period },
            )
            .map_err(|e| unavailable(&e.to_string()))?;

        let events = client.set_get_eventhandle().map_err(|e| e.to_string())?;
        let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
        let capacity = client.get_buffer_size().map_err(|e| e.to_string())? as usize;
        let channels = usize::from(source.channels);
        let block_align = format.get_blockalign() as usize;
        Ok(Session {
            client,
            render,
            events,
            sample,
            channels,
            block_align,
            latency: Duration::from_secs_f64(capacity as f64 / f64::from(source.sample_rate)),
            scratch: Scratch {
                ints16: vec![0; capacity * channels],
                ints32: vec![0; capacity * channels],
                floats: vec![0.0; capacity * channels],
            },
            bytes: vec![0; capacity * block_align],
        })
    }

    /// Hands the device as many frames as it has room for.
    fn fill(&mut self, callback: &mut Callback) -> std::result::Result<(), String> {
        let frames = self
            .client
            .get_available_space_in_frames()
            .map_err(|e| e.to_string())? as usize;
        if frames == 0 {
            return Ok(());
        }
        self.render_frames(callback, frames);
        self.render
            .write_to_device(frames, &self.bytes[..frames * self.block_align], None)
            .map_err(|e| e.to_string())
    }

    fn run(
        &mut self,
        callback: &mut Callback,
        control: &Control,
    ) -> std::result::Result<(), String> {
        while !control.stop.load(Ordering::Acquire) {
            self.events
                .wait_for_event(EVENT_TIMEOUT_MS)
                .map_err(|_| "the device stopped asking for audio".to_owned())?;
            self.fill(callback)?;
        }
        Ok(())
    }

    /// Renders `frames` frames from the callback into `self.bytes` in the device's layout.
    fn render_frames(&mut self, callback: &mut Callback, frames: usize) {
        let samples = frames * self.channels;
        let Sample {
            container_bits,
            float,
            ..
        } = self.sample;
        match (container_bits, float) {
            (16, _) => {
                let buffer = &mut self.scratch.ints16[..samples];
                callback.render(buffer);
                for (sample, bytes) in buffer.iter().zip(self.bytes.chunks_exact_mut(2)) {
                    bytes.copy_from_slice(&sample.to_le_bytes());
                }
            }
            (24, _) => {
                let buffer = &mut self.scratch.ints32[..samples];
                callback.render(buffer);
                pack_24_bit(buffer, &mut self.bytes);
            }
            (_, true) => {
                let buffer = &mut self.scratch.floats[..samples];
                callback.render(buffer);
                for (sample, bytes) in buffer.iter().zip(self.bytes.chunks_exact_mut(4)) {
                    bytes.copy_from_slice(&sample.to_le_bytes());
                }
            }
            _ => {
                let buffer = &mut self.scratch.ints32[..samples];
                callback.render(buffer);
                for (sample, bytes) in buffer.iter().zip(self.bytes.chunks_exact_mut(4)) {
                    bytes.copy_from_slice(&sample.to_le_bytes());
                }
            }
        }
    }
}
