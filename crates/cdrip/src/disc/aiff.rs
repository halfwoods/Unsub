//! Sequential reader for the AIFF files cddafs presents for each audio track: 16-bit big-endian
//! stereo at 44.1 kHz.

use std::{
    fs::File,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

use super::reader::TrackAudio;
use crate::{Error, Result};

pub(crate) struct AiffAudio {
    path: PathBuf,
    reader: BufReader<File>,
    frames: u64,
    frames_left: u64,
    bytes: Vec<u8>,
}

pub(crate) fn open(path: &Path) -> Result<AiffAudio> {
    let io_err = |err| Error::io(path, err);
    let mut reader = BufReader::with_capacity(1 << 16, File::open(path).map_err(io_err)?);
    let header = parse_header(&mut reader).map_err(|err| match err {
        HeaderError::Io(err) => io_err(err),
        HeaderError::Format(detail) => Error::Disc(format!("{}: {detail}", path.display())),
    })?;
    Ok(AiffAudio {
        path: path.to_owned(),
        reader,
        frames: header.frames,
        frames_left: header.frames,
        bytes: Vec::new(),
    })
}

impl TrackAudio for AiffAudio {
    fn frames(&self) -> u64 {
        self.frames
    }

    fn read(&mut self, out: &mut [i16]) -> Result<usize> {
        let frames = (out.len() / 2).min(usize::try_from(self.frames_left).unwrap_or(usize::MAX));
        if frames == 0 {
            return Ok(0);
        }
        self.bytes.resize(frames * 4, 0);
        self.reader
            .read_exact(&mut self.bytes)
            .map_err(|err| match err.kind() {
                io::ErrorKind::UnexpectedEof => {
                    Error::Disc(format!("{} ends early", self.path.display()))
                }
                _ => Error::io(&self.path, err),
            })?;
        for (sample, bytes) in out.iter_mut().zip(self.bytes.chunks_exact(2)) {
            *sample = i16::from_be_bytes([bytes[0], bytes[1]]);
        }
        self.frames_left -= frames as u64;
        Ok(frames)
    }
}

struct Header {
    frames: u64,
}

enum HeaderError {
    Io(io::Error),
    Format(String),
}

impl From<io::Error> for HeaderError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Reads up to the first byte of sample data, leaving the reader there.
fn parse_header(reader: &mut impl Read) -> std::result::Result<Header, HeaderError> {
    let format = |detail: &str| HeaderError::Format(detail.to_owned());
    let mut form = [0u8; 12];
    reader.read_exact(&mut form)?;
    if &form[..4] != b"FORM" || !matches!(&form[8..], b"AIFF" | b"AIFC") {
        return Err(format("not an AIFF file"));
    }

    let mut comm_frames = None;
    loop {
        let mut chunk = [0u8; 8];
        reader.read_exact(&mut chunk)?;
        let size = u64::from(u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        match &chunk[..4] {
            b"COMM" => {
                let mut comm =
                    vec![0u8; usize::try_from(size).map_err(|_| format("oversized COMM chunk"))?];
                reader.read_exact(&mut comm)?;
                if comm.len() < 18 {
                    return Err(format("short COMM chunk"));
                }
                let channels = u16::from_be_bytes([comm[0], comm[1]]);
                let bits = u16::from_be_bytes([comm[6], comm[7]]);
                let rate = extended_to_f64(&comm[8..18]);
                if (channels, bits) != (2, 16) || (rate - 44_100.0).abs() > 0.5 {
                    return Err(HeaderError::Format(format!(
                        "expected 16-bit stereo at 44.1 kHz, found {channels} channels, {bits} bits, {rate} Hz"
                    )));
                }
                if comm.len() >= 22 && &comm[18..22] != b"NONE" {
                    return Err(format("compressed AIFF-C"));
                }
                comm_frames = Some(u64::from(u32::from_be_bytes([
                    comm[2], comm[3], comm[4], comm[5],
                ])));
            }
            b"SSND" => {
                let frames = comm_frames.ok_or_else(|| format("SSND before COMM"))?;
                let mut offsets = [0u8; 8];
                reader.read_exact(&mut offsets)?;
                let skip = u64::from(u32::from_be_bytes([
                    offsets[0], offsets[1], offsets[2], offsets[3],
                ]));
                io::copy(&mut reader.by_ref().take(skip), &mut io::sink())?;
                // The header's frame count wins unless the chunk is shorter, which would mean a
                // file that was cut off.
                let in_chunk = size.saturating_sub(8 + skip) / 4;
                return Ok(Header {
                    frames: frames.min(in_chunk),
                });
            }
            _ => {
                // Chunks are padded to an even length.
                io::copy(
                    &mut reader.by_ref().take(size + (size & 1)),
                    &mut io::sink(),
                )?;
            }
        }
    }
}

/// The 80-bit IEEE extended float AIFF uses for the sample rate.
fn extended_to_f64(bytes: &[u8]) -> f64 {
    let exponent = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]) & 0x7FFF) - 16_383;
    let mantissa = u64::from_be_bytes([
        bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
    ]);
    mantissa as f64 * 2f64.powi(exponent - 63)
}

/// Builds an AIFF file's bytes. Only tests need to write the format.
#[cfg(test)]
pub(crate) fn encode(samples: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap();
    let mut out = Vec::new();
    out.extend(b"FORM");
    out.extend((4 + 8 + 18 + 8 + 8 + data_len).to_be_bytes());
    out.extend(b"AIFF");
    out.extend(b"COMM");
    out.extend(18u32.to_be_bytes());
    out.extend(2u16.to_be_bytes());
    out.extend((data_len / 4).to_be_bytes());
    out.extend(16u16.to_be_bytes());
    // 44100 as an 80-bit extended float.
    out.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
    out.extend(b"SSND");
    out.extend((8 + data_len).to_be_bytes());
    out.extend([0u8; 8]);
    out.extend(samples.iter().flat_map(|sample| sample.to_be_bytes()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(bytes: &[u8]) -> Result<Vec<i16>> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1 Audio Track.aiff");
        std::fs::write(&path, bytes).unwrap();
        let mut audio = open(&path)?;
        let mut all = Vec::new();
        let mut buf = [0i16; 100];
        loop {
            let frames = audio.read(&mut buf)?;
            if frames == 0 {
                return Ok(all);
            }
            all.extend(&buf[..frames * 2]);
        }
    }

    #[test]
    fn reads_big_endian_stereo_samples_in_chunks() {
        let samples: Vec<i16> = (0..1000).map(|n| (n * 37 - 18_000) as i16).collect();
        assert_eq!(read_all(&encode(&samples)).unwrap(), samples);
    }

    #[test]
    fn sample_rate_decodes_from_extended_precision() {
        assert_eq!(
            extended_to_f64(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]),
            44_100.0
        );
        assert_eq!(
            extended_to_f64(&[0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]),
            48_000.0
        );
    }

    #[test]
    fn skips_unknown_chunks_before_the_audio() {
        let mut bytes = encode(&[1, 2, 3, 4]);
        // Insert an odd-sized "ANNO" chunk (with its pad byte) between FORM and COMM.
        let annotation = [b"ANNO".as_slice(), &3u32.to_be_bytes(), b"hi!", &[0]].concat();
        bytes.splice(12..12, annotation);
        assert_eq!(read_all(&bytes).unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(matches!(read_all(b"RIFF....WAVEfmt "), Err(Error::Disc(_))));
        let mut mono = encode(&[1, 2]);
        mono[20..22].copy_from_slice(&1u16.to_be_bytes());
        assert!(matches!(read_all(&mono), Err(Error::Disc(_))));
    }

    #[test]
    fn a_truncated_file_is_an_error_not_silence() {
        let mut bytes = encode(&vec![7i16; 400]);
        bytes.truncate(bytes.len() - 100);
        // The SSND chunk now claims more than the file holds.
        assert!(read_all(&bytes).is_err());
    }
}
