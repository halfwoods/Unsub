//! A read-only copy of what the output callback plays, for the visualizer.
//!
//! The callback writes each block it renders into a pair of single-producer, single-consumer
//! rings (the samples, and a note of when each block will be heard) and a [`Tap`] reads them back
//! on another thread. Nothing here ever waits: when no tap exists the callback pays for one
//! relaxed load, and when the reader falls behind a block is dropped, not queued.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rtrb::chunks::WriteChunk;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::output::Format;

/// Audio the tap holds for a slow reader. A reader polling at 60 Hz needs a fraction of this.
const TAP_MS: usize = 500;

/// Blocks the tap can hold; callbacks of 64 frames at 192 kHz still fit the half second.
const MAX_BLOCKS: usize = 2_048;

/// One block of output as the callback rendered it.
struct Span {
    /// When the block's first sample reaches the speakers, on the tap's clock.
    at: Duration,
    samples: usize,
}

/// The rings of the stream currently open, and the format it runs at.
struct Feed {
    samples: Consumer<f32>,
    spans: Consumer<Span>,
    format: Format,
}

/// What the output callbacks and the [`Tap`]s share. A new output stream takes the feed over
/// from the last one, so a tap follows the player across device changes and reopens.
pub struct TapHub {
    epoch: Instant,
    readers: AtomicUsize,
    dropped: AtomicU64,
    feed: Mutex<Option<Feed>>,
}

impl Default for TapHub {
    fn default() -> Self {
        TapHub {
            epoch: Instant::now(),
            readers: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            feed: Mutex::new(None),
        }
    }
}

impl TapHub {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn feed(&self) -> MutexGuard<'_, Option<Feed>> {
        self.feed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The write end for a new output stream at `format`. Whatever the last stream left unread
    /// is forgotten.
    pub(crate) fn writer(self: &Arc<Self>, format: Format) -> TapWriter {
        let capacity = format.sample_rate as usize * usize::from(format.channels) * TAP_MS / 1000;
        let (samples_in, samples) = RingBuffer::new(capacity);
        let (spans_in, spans) = RingBuffer::new(MAX_BLOCKS);
        *self.feed() = Some(Feed {
            samples,
            spans,
            format,
        });
        TapWriter {
            hub: Arc::clone(self),
            samples: samples_in,
            spans: spans_in,
        }
    }

    /// Throws away everything queued. Blocks are counted off first and their samples after, so a
    /// block the callback finishes meanwhile is never left half-removed.
    fn discard(&self) {
        let mut guard = self.feed();
        let Some(feed) = guard.as_mut() else { return };
        let mut samples = 0;
        while let Ok(span) = feed.spans.pop() {
            samples += span.samples;
        }
        if let Ok(chunk) = feed.samples.read_chunk(samples) {
            chunk.commit_all();
        }
    }
}

/// The audio callback's end of the tap. Real-time safe: no locks, allocation or I/O.
pub(crate) struct TapWriter {
    hub: Arc<TapHub>,
    samples: Producer<f32>,
    spans: Producer<Span>,
}

impl TapWriter {
    /// Room for a block of at most `samples` samples that will be heard `latency` from now, or
    /// `None` when nobody is reading or the reader has fallen behind (in which case the block
    /// is dropped and counted).
    pub(crate) fn capture(&mut self, samples: usize, latency: Duration) -> Option<Capture<'_>> {
        if self.hub.readers.load(Ordering::Relaxed) == 0 {
            return None;
        }
        // Only the reader frees blocks, so a free one stays free until `finish`.
        let chunk = if self.spans.is_full() {
            None
        } else {
            self.samples.write_chunk(samples).ok()
        };
        let Some(chunk) = chunk else {
            self.hub.dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        Some(Capture {
            chunk,
            spans: &mut self.spans,
            at: self.hub.now() + latency,
            written: 0,
        })
    }
}

/// One block being copied out.
pub(crate) struct Capture<'a> {
    chunk: WriteChunk<'a, f32>,
    spans: &'a mut Producer<Span>,
    at: Duration,
    written: usize,
}

impl Capture<'_> {
    pub(crate) fn push(&mut self, sample: f32) {
        let (head, tail) = self.chunk.as_mut_slices();
        let slot = match head.get_mut(self.written) {
            Some(slot) => Some(slot),
            None => tail.get_mut(self.written - head.len()),
        };
        if let Some(slot) = slot {
            *slot = sample;
            self.written += 1;
        }
    }

    /// Publishes the samples pushed, once the reader can also see when they play.
    pub(crate) fn finish(self) {
        if self.written == 0 {
            return;
        }
        self.chunk.commit(self.written);
        // A free block was checked for in `capture`, so this cannot fail.
        let _ = self.spans.push(Span {
            at: self.at,
            samples: self.written,
        });
    }
}

/// Some audio as it was sent to the output, ahead of the volume.
pub struct TapBlock<'a> {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved, after the equalizer and before the volume, so the music is as loud as it was
    /// mixed however the volume knob is set. Fades for pausing and flushing are in it.
    pub samples: &'a [f32],
    /// When the first sample is heard, on [`Tap::now`]'s clock: the time the output callback ran
    /// plus the latency the audio host reports between that and the speakers.
    pub at: Duration,
}

/// A read-only view of the player's output, from [`crate::Player::tap`].
///
/// Reading never slows the audio: when it falls behind, blocks are dropped. The samples are
/// copies, so bit-perfect output stays bit-perfect. Several taps at once would split the blocks
/// between them, so keep one. Dropping the tap stops the copying.
pub struct Tap {
    hub: Arc<TapHub>,
    scratch: Vec<f32>,
}

impl Tap {
    pub(crate) fn new(hub: Arc<TapHub>) -> Tap {
        hub.readers.fetch_add(1, Ordering::Relaxed);
        hub.discard();
        Tap {
            hub,
            scratch: Vec::new(),
        }
    }

    /// The clock [`TapBlock::at`] is on: monotonic, counting from when the player was made. Ask
    /// the analyzer for the frame at this time to get the one being heard.
    pub fn now(&self) -> Duration {
        self.hub.now()
    }

    /// The oldest block not yet read, or `None` when caught up. The format can change from one
    /// block to the next when a new song or output device needs it; the audio before and after
    /// is then unrelated.
    pub fn read(&mut self) -> Option<TapBlock<'_>> {
        let mut guard = self.hub.feed();
        let feed = guard.as_mut()?;
        let span = feed.spans.pop().ok()?;
        let chunk = feed.samples.read_chunk(span.samples).ok()?;
        let (head, tail) = chunk.as_slices();
        self.scratch.clear();
        self.scratch.extend_from_slice(head);
        self.scratch.extend_from_slice(tail);
        chunk.commit_all();
        Some(TapBlock {
            sample_rate: feed.format.sample_rate,
            channels: feed.format.channels,
            samples: &self.scratch,
            at: span.at,
        })
    }

    /// How many blocks have been dropped because nobody was reading fast enough.
    pub fn dropped(&self) -> u64 {
        self.hub.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.hub.readers.fetch_sub(1, Ordering::Relaxed);
    }
}
