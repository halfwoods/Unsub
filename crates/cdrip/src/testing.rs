//! Shared fixtures for the unit tests: a simulated disc, synthetic audio and a scripted network.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    Error, MetadataClient, Result,
    disc::{aiff, plist},
};

/// .NET's `System.Random(seed)` byte stream (the legacy subtractive generator), which CUETools'
/// test helper used to synthesise its audio.
pub(crate) struct DotNetRandom {
    seeds: [i32; 56],
    next: usize,
    next_p: usize,
}

impl DotNetRandom {
    pub fn new(seed: i32) -> Self {
        const MBIG: i32 = i32::MAX;
        let mut seeds = [0i32; 56];
        let mut j = 161_803_398 - seed.abs();
        seeds[55] = j;
        let mut k = 1;
        for i in 1..55 {
            let ii = (21 * i) % 55;
            seeds[ii] = k;
            k = j - k;
            if k < 0 {
                k += MBIG;
            }
            j = seeds[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                seeds[i] = seeds[i].wrapping_sub(seeds[1 + (i + 30) % 55]);
                if seeds[i] < 0 {
                    seeds[i] += MBIG;
                }
            }
        }
        Self {
            seeds,
            next: 0,
            next_p: 21,
        }
    }

    pub fn next_byte(&mut self) -> u8 {
        self.next = if self.next + 1 >= 56 {
            1
        } else {
            self.next + 1
        };
        self.next_p = if self.next_p + 1 >= 56 {
            1
        } else {
            self.next_p + 1
        };
        let mut value = self.seeds[self.next].wrapping_sub(self.seeds[self.next_p]);
        if value == i32::MAX {
            value -= 1;
        }
        if value < 0 {
            value += i32::MAX;
        }
        self.seeds[self.next] = value;
        (value % 256) as u8
    }

    pub fn samples(&mut self, frames: usize) -> Vec<i16> {
        (0..frames * 2)
            .map(|_| i16::from_le_bytes([self.next_byte(), self.next_byte()]))
            .collect()
    }
}

/// Deterministic audio that is neither silent nor incompressible: two detuned tones plus a little
/// noise. `seed` varies it from track to track.
pub(crate) fn synthetic_audio(seed: u32, frames: usize) -> Vec<i16> {
    let mut noise = u64::from(seed)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(1);
    (0..frames)
        .flat_map(|frame| {
            noise = noise
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let jitter = ((noise >> 33) % 400) as f64 - 200.0;
            let t = frame as f64 / 44_100.0;
            let left = 9_000.0
                * (t * 440.0 * (1.0 + f64::from(seed) / 10.0) * std::f64::consts::TAU).sin()
                + jitter;
            let right = 7_000.0 * (t * 660.0 * std::f64::consts::TAU).sin() - jitter;
            [left as i16, right as i16]
        })
        .collect()
}

/// A volume laid out like cddafs: `.TOC.plist` plus `<n> Audio Track.aiff`.
pub(crate) fn make_volume(
    root: &Path,
    name: &str,
    first: u8,
    lead_out: u32,
    starts: &[u32],
    data: &[bool],
) -> PathBuf {
    let mount = root.join(name);
    fs::create_dir_all(&mount).unwrap();
    fs::write(
        mount.join(".TOC.plist"),
        plist::xml(first, lead_out, starts, data),
    )
    .unwrap();
    for (number, (start, next)) in
        (first..).zip(starts.iter().zip(starts.iter().skip(1).chain([&lead_out])))
    {
        if data
            .get(usize::from(number - first))
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        let frames = ((next - start) as usize).min(2) * 588;
        fs::write(
            mount.join(format!("{number} Audio Track.aiff")),
            aiff::encode(&vec![0i16; frames * 2]),
        )
        .unwrap();
    }
    mount
}

/// Like `make_volume`, but each audio track gets the given interleaved samples.
pub(crate) fn make_volume_with_audio(
    root: &Path,
    name: &str,
    lead_out: u32,
    starts: &[u32],
    audio: &[Vec<i16>],
) -> PathBuf {
    let mount = root.join(name);
    fs::create_dir_all(&mount).unwrap();
    fs::write(
        mount.join(".TOC.plist"),
        plist::xml(1, lead_out, starts, &[]),
    )
    .unwrap();
    for (number, samples) in (1..).zip(audio) {
        fs::write(
            mount.join(format!("{number} Audio Track.aiff")),
            aiff::encode(samples),
        )
        .unwrap();
    }
    mount
}

/// Answers exact URLs from a table and records every request. A URL with no entry is an error, so a
/// test notices a request it did not expect.
#[derive(Default)]
pub(crate) struct FakeClient {
    routes: HashMap<String, Option<Vec<u8>>>,
    pub requests: Mutex<Vec<String>>,
}

impl FakeClient {
    pub fn with(mut self, url: &str, body: &[u8]) -> Self {
        self.routes.insert(url.to_owned(), Some(body.to_vec()));
        self
    }

    /// The server answers 404 for `url`.
    pub fn without(mut self, url: &str) -> Self {
        self.routes.insert(url.to_owned(), None);
        self
    }

    pub fn requested(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl MetadataClient for FakeClient {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        self.requests.lock().unwrap().push(url.to_owned());
        self.routes
            .get(url)
            .cloned()
            .ok_or_else(|| Error::Network(format!("no route for {url}")))
    }
}

#[cfg(target_os = "macos")]
/// Decodes an M4A (ALAC or AAC) with symphonia, a decoder independent of the encoder under test.
/// Returns interleaved 16-bit samples, which are exact for ALAC.
pub(crate) fn decode_m4a(path: &Path) -> Vec<i16> {
    use symphonia::core::{
        codecs::audio::AudioDecoderOptions,
        formats::{FormatOptions, TrackType, probe::Hint},
        io::MediaSourceStream,
        meta::MetadataOptions,
    };

    let source =
        MediaSourceStream::new(Box::new(fs::File::open(path).unwrap()), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("m4a");
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .unwrap();
    let track = format.default_track(TrackType::Audio).unwrap();
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .unwrap();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .unwrap();

    let mut samples = Vec::new();
    let mut chunk: Vec<f32> = Vec::new();
    while let Some(packet) = format.next_packet().unwrap() {
        if packet.track_id != track_id {
            continue;
        }
        decoder
            .decode(&packet)
            .unwrap()
            .copy_to_vec_interleaved(&mut chunk);
        samples.extend(
            chunk
                .iter()
                .map(|sample| (sample * 32_768.0).round() as i16),
        );
    }
    samples
}
