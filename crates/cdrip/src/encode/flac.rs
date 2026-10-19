use std::{fs, path::Path};

use flacenc::{
    bitsink::ByteSink,
    component::BitRepr,
    config::Encoder,
    error::{SourceError, Verify},
    source::{Fill, Source},
};

use crate::{Error, Result, disc::TrackAudio};

const CHANNELS: usize = 2;
const BITS_PER_SAMPLE: usize = 16;
const SAMPLE_RATE: usize = 44_100;
/// FLAC's "subset" (what hardware players decode) allows at most 12 predictor coefficients at
/// 44.1 kHz and blocks up to 4608 samples.
const MAX_SUBSET_ORDER: usize = 12;
/// Predictor order for compression levels 1 to 8; level 0 uses only the fixed predictors.
const LPC_ORDERS: [usize; 8] = [3, 5, 6, 8, 10, 12, 12, 12];

pub(crate) fn encode(compression: u8, audio: &mut dyn TrackAudio, out: &Path) -> Result<()> {
    let config = config(compression)
        .into_verified()
        .map_err(|(_, err)| Error::Encode(err.to_string()))?;
    let mut source = AudioSource {
        audio,
        failure: None,
        pcm: Vec::new(),
        widened: Vec::new(),
    };
    let stream =
        match flacenc::encode_with_fixed_block_size(&config, &mut source, config.block_size) {
            Ok(stream) => stream,
            // The source's own error (cancellation, a read failure) is more useful than flacenc's wrapper.
            Err(err) => {
                return Err(source
                    .failure
                    .take()
                    .unwrap_or_else(|| Error::Encode(err.to_string())));
            }
        };
    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|err| Error::Encode(err.to_string()))?;
    fs::write(out, sink.as_slice()).map_err(|err| Error::io(out, err))
}

fn config(compression: u8) -> Encoder {
    let mut config = Encoder::default();
    match usize::from(compression.min(8)) {
        0 => config.subframe_coding.use_lpc = false,
        level => {
            config.subframe_coding.qlpc.lpc_order = LPC_ORDERS[level - 1].min(MAX_SUBSET_ORDER)
        }
    }
    config
}

/// Feeds flacenc from a track, one block at a time, so a track is never held in memory as PCM.
struct AudioSource<'a> {
    audio: &'a mut dyn TrackAudio,
    /// The error that stopped `audio`, kept because flacenc's `SourceError` cannot carry it.
    failure: Option<Error>,
    pcm: Vec<i16>,
    widened: Vec<i32>,
}

impl Source for AudioSource<'_> {
    fn channels(&self) -> usize {
        CHANNELS
    }

    fn bits_per_sample(&self) -> usize {
        BITS_PER_SAMPLE
    }

    fn sample_rate(&self) -> usize {
        SAMPLE_RATE
    }

    fn read_samples<F: Fill>(
        &mut self,
        block_size: usize,
        dest: &mut F,
    ) -> std::result::Result<usize, SourceError> {
        self.pcm.resize(block_size * CHANNELS, 0);
        let mut frames = 0;
        while frames < block_size {
            match self.audio.read(&mut self.pcm[frames * CHANNELS..]) {
                Ok(0) => break,
                Ok(read) => frames += read,
                Err(err) => {
                    self.failure = Some(err);
                    return Err(SourceError::from_unknown());
                }
            }
        }
        if frames == 0 {
            return Ok(0);
        }
        self.widened.clear();
        self.widened.extend(
            self.pcm[..frames * CHANNELS]
                .iter()
                .map(|&sample| i32::from(sample)),
        );
        dest.fill_interleaved(&self.widened)?;
        Ok(frames)
    }
}
