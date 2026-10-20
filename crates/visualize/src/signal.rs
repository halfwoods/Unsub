//! The most recent audio, kept per channel in rings, with the clock that says when each sample is
//! heard. A run of audio that follows on from the last block is one segment; a jump in the
//! presentation time (a seek, a new song, a dropout) starts another, whose samples never mix with
//! the old ones.

pub(crate) struct Signal {
    channels: usize,
    /// Each ring is stored twice over, so any stretch up to its capacity is one contiguous slice.
    left: Vec<f32>,
    right: Vec<f32>,
    mono: Vec<f32>,
    capacity: usize,
    sample_rate: f64,
    /// Sample-frames written since creation; the next one gets this index.
    total: u64,
    segment_start: u64,
    /// The presentation time of sample `segment_start`, in seconds.
    origin: f64,
    open: bool,
}

impl Signal {
    /// `capacity` sample-frames are kept per channel, rounded up to a power of two.
    pub(crate) fn new(sample_rate: u32, channels: usize, capacity: usize) -> Self {
        let capacity = capacity.next_power_of_two();
        Self {
            channels,
            left: vec![0.0; 2 * capacity],
            right: vec![0.0; 2 * capacity],
            mono: vec![0.0; 2 * capacity],
            capacity,
            sample_rate: f64::from(sample_rate),
            total: 0,
            segment_start: 0,
            origin: 0.0,
            open: false,
        }
    }

    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    pub(crate) fn segment_start(&self) -> u64 {
        self.segment_start
    }

    /// Whether any segment has begun since the last [`Signal::close`].
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    pub(crate) fn begin_segment(&mut self, at: f64) {
        self.segment_start = self.total;
        self.origin = at;
        self.open = true;
    }

    /// How far `at`, the time the next sample is heard, is from where the segment's clock puts it.
    pub(crate) fn drift(&self, at: f64) -> f64 {
        at - self.time_of(self.total)
    }

    /// Moves the segment's clock a little towards the presentation time, so a device clock that
    /// runs slightly fast or slow does not pile up error, while jitter between blocks averages out.
    pub(crate) fn follow(&mut self, drift: f64) {
        self.origin += drift * 0.1;
    }

    pub(crate) fn time_of(&self, index: u64) -> f64 {
        self.origin + (index as f64 - self.segment_start as f64) / self.sample_rate
    }

    /// Appends whole sample-frames of interleaved audio. Non-finite samples become silence.
    pub(crate) fn write(&mut self, interleaved: &[f32]) {
        let channels = self.channels;
        for frame in interleaved.chunks_exact(channels) {
            let clean = |sample: f32| if sample.is_finite() { sample } else { 0.0 };
            let left = clean(frame[0]);
            let right = if channels > 1 { clean(frame[1]) } else { left };
            let mono = if channels > 2 {
                frame.iter().map(|&sample| clean(sample)).sum::<f32>() / channels as f32
            } else {
                (left + right) * 0.5
            };
            let slot = self.total as usize & (self.capacity - 1);
            for copy in [slot, slot + self.capacity] {
                self.left[copy] = left;
                self.right[copy] = right;
                self.mono[copy] = mono;
            }
            self.total += 1;
        }
    }

    /// `len` mono samples starting at sample `start`.
    pub(crate) fn mono(&self, start: u64, len: usize) -> &[f32] {
        self.stretch(&self.mono, start, len)
    }

    pub(crate) fn left(&self, start: u64, len: usize) -> &[f32] {
        self.stretch(&self.left, start, len)
    }

    pub(crate) fn right(&self, start: u64, len: usize) -> &[f32] {
        self.stretch(&self.right, start, len)
    }

    fn stretch<'a>(&self, ring: &'a [f32], start: u64, len: usize) -> &'a [f32] {
        let at = start as usize & (self.capacity - 1);
        &ring[at..at + len]
    }

    /// Mean of the squares of both channels over `len` samples starting at `start`.
    pub(crate) fn mean_square(&self, start: u64, len: usize) -> f32 {
        let squares = |samples: &[f32]| samples.iter().map(|sample| sample * sample).sum::<f32>();
        (squares(self.left(start, len)) + squares(self.right(start, len))) / (2 * len) as f32
    }
}
