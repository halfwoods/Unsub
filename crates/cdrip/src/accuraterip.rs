//! AccurateRip: disc ids, per-track checksums and the database's binary answer format.
//!
//! Everything here follows the reference implementation in CUETools (`AccurateRip.cs`), whose own
//! test suite supplies the checksum test vectors used below.

use crate::{
    Error, Result,
    http::MetadataClient,
    toc::{FRAMES_PER_SECTOR, LEAD_IN_SECTORS, Toc, cddb_disc_id},
};

/// What the AccurateRip database says about a ripped track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccurateRip {
    /// Another ripper got the same checksum; `confidence` is how many.
    Matched { confidence: u32 },
    /// The database has no entry for this disc (or track).
    NotInDatabase,
    /// The disc is known but this rip's checksum matches none of its pressings. With a drive whose
    /// read offset is not zero this is expected: the checksums depend on the exact sample alignment.
    Mismatch,
    /// Verification was off or could not be done (offline, data tracks on the disc, ...).
    Skipped,
}

/// The audio on either end of a disc is not covered by the checksum: the first track skips its
/// first five sectors less one sample and the last track its final five sectors, so that drives
/// with different read offsets still agree.
const EDGE_SAMPLES: u64 = 5 * FRAMES_PER_SECTOR as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiscIds {
    pub id1: u32,
    pub id2: u32,
    pub cddb: u32,
}

/// The ids use sectors counted from the start of the program area, without the 150 sector lead-in
/// that `Toc` includes.
pub(crate) fn disc_ids(toc: &Toc) -> DiscIds {
    let starts = || {
        toc.boundaries()
            .map(|sector| sector.saturating_sub(LEAD_IN_SECTORS))
    };
    let id1 = starts().fold(0u32, u32::wrapping_add);
    let id2 = starts().zip(1u32..).fold(0u32, |sum, (sector, position)| {
        sum.wrapping_add(sector.max(1).wrapping_mul(position))
    });
    DiscIds {
        id1,
        id2,
        cddb: cddb_disc_id(toc),
    }
}

pub(crate) fn database_url(toc: &Toc) -> String {
    let ids = disc_ids(toc);
    // Entries are spread over directories named after the id's last three hex digits, reversed.
    format!(
        "https://www.accuraterip.com/accuraterip/{:x}/{:x}/{:x}/dBAR-{:03}-{:08x}-{:08x}-{:08x}.bin",
        ids.id1 & 0xF,
        ids.id1 >> 4 & 0xF,
        ids.id1 >> 8 & 0xF,
        toc.track_count(),
        ids.id1,
        ids.id2,
        ids.cddb,
    )
}

/// Running v1 and v2 checksums of one track's samples (16-bit stereo, 44.1 kHz).
pub(crate) struct Checksum {
    position: u64,
    first: u64,
    last: u64,
    v1: u32,
    v2_high: u32,
}

impl Checksum {
    /// `frames` is the track's length in stereo samples. Positions `first..=last` (1-based) count.
    pub fn new(frames: u64, is_first_track: bool, is_last_track: bool) -> Self {
        Self {
            position: 0,
            first: if is_first_track { EDGE_SAMPLES } else { 1 },
            last: if is_last_track {
                frames.saturating_sub(EDGE_SAMPLES)
            } else {
                u64::MAX
            },
            v1: 0,
            v2_high: 0,
        }
    }

    /// `samples` is interleaved left/right.
    pub fn update(&mut self, samples: &[i16]) {
        for pair in samples.chunks_exact(2) {
            self.position += 1;
            if self.position < self.first || self.position > self.last {
                continue;
            }
            let word = u32::from(pair[0] as u16) | u32::from(pair[1] as u16) << 16;
            let product = u64::from(word) * self.position;
            self.v1 = self.v1.wrapping_add(product as u32);
            self.v2_high = self.v2_high.wrapping_add((product >> 32) as u32);
        }
    }

    pub fn finish(&self) -> Sums {
        Sums {
            v1: self.v1,
            v2: self.v1.wrapping_add(self.v2_high),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sums {
    pub v1: u32,
    pub v2: u32,
}

/// One pressing's record from the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pressing {
    pub ids: DiscIds,
    pub tracks: Vec<PressedTrack>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PressedTrack {
    pub confidence: u8,
    pub checksum: u32,
    /// Checksum of sector 450, which rippers use to detect a drive's offset. Unused here.
    pub frame_450: u32,
}

/// A `dBAR` file: for each pressing, 13 header bytes (track count, then three little-endian ids)
/// followed by 9 bytes per track (confidence, checksum, frame 450 checksum).
pub(crate) fn parse_database(mut bytes: &[u8]) -> Result<Vec<Pressing>> {
    let truncated = || Error::Response {
        service: "AccurateRip",
        detail: "truncated database record".into(),
    };
    let word = |bytes: &[u8]| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let mut pressings = Vec::new();
    while !bytes.is_empty() {
        let (header, rest) = bytes.split_at_checked(13).ok_or_else(truncated)?;
        let count = usize::from(header[0]);
        let (records, rest) = rest.split_at_checked(count * 9).ok_or_else(truncated)?;
        pressings.push(Pressing {
            ids: DiscIds {
                id1: word(&header[1..]),
                id2: word(&header[5..]),
                cddb: word(&header[9..]),
            },
            tracks: records
                .chunks_exact(9)
                .map(|record| PressedTrack {
                    confidence: record[0],
                    checksum: word(&record[1..]),
                    frame_450: word(&record[5..]),
                })
                .collect(),
        });
        bytes = rest;
    }
    Ok(pressings)
}

/// Asks the database about a disc. `Ok(vec![])` means it is not listed.
pub(crate) fn fetch(toc: &Toc, client: &dyn MetadataClient) -> Result<Vec<Pressing>> {
    client
        .get(&database_url(toc))?
        .map_or(Ok(Vec::new()), |bytes| parse_database(&bytes))
}

/// Compares one track's checksums with every pressing in the database. Either checksum version may
/// match, as the database holds both.
pub(crate) fn verdict(pressings: &[Pressing], track_index: usize, sums: Sums) -> AccurateRip {
    let entries = || {
        pressings
            .iter()
            .filter_map(|pressing| pressing.tracks.get(track_index))
    };
    let confidence = entries()
        .filter(|entry| entry.checksum == sums.v1 || entry.checksum == sums.v2)
        .map(|entry| u32::from(entry.confidence))
        .max();
    match confidence {
        Some(confidence) => AccurateRip::Matched { confidence },
        None if entries().next().is_some() => AccurateRip::Mismatch,
        None => AccurateRip::NotInDatabase,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::DotNetRandom;

    /// CUETools' `AccurateRipVerifyTest`: layout "13 68 99 136" (a 13 sector pregap, then tracks of
    /// 55, 31 and 37 sectors) filled from `Random(2314)`, with the checksums its asserts expect.
    #[test]
    fn reproduces_the_cuetools_reference_checksums() {
        let mut random = DotNetRandom::new(2314);
        let _pregap = random.samples(13 * FRAMES_PER_SECTOR as usize);
        let lengths = [55u64, 31, 37];
        let expected = [
            (3_727_147_246, 3_988_391_122),
            (2_202_235_240, 2_284_845_104),
            (3_752_629_998, 3_841_231_027),
        ];
        for (index, (&sectors, &(v1, v2))) in lengths.iter().zip(&expected).enumerate() {
            let frames = sectors * u64::from(FRAMES_PER_SECTOR);
            let mut checksum = Checksum::new(frames, index == 0, index == 2);
            checksum.update(&random.samples(frames as usize));
            assert_eq!(checksum.finish(), Sums { v1, v2 }, "track {}", index + 1);
        }
    }

    #[test]
    fn checksum_is_independent_of_how_the_samples_are_chunked() {
        let samples = DotNetRandom::new(7).samples(20_000);
        let mut whole = Checksum::new(20_000, true, true);
        whole.update(&samples);
        let mut pieces = Checksum::new(20_000, true, true);
        for chunk in samples.chunks(1234 * 2) {
            pieces.update(chunk);
        }
        assert_eq!(whole.finish(), pieces.finish());
    }

    /// Worked by hand: three frames of (1, 2) -> word 0x0002_0001. With no skipping, positions
    /// 1, 2, 3 give products 0x20001, 0x40002, 0x60003; none exceed 32 bits, so v1 is their sum
    /// 0xC0006 and v2 equals it.
    #[test]
    fn small_case_by_hand() {
        let mut checksum = Checksum::new(3, false, false);
        checksum.update(&[1, 2, 1, 2, 1, 2]);
        assert_eq!(
            checksum.finish(),
            Sums {
                v1: 0x000C_0006,
                v2: 0x000C_0006
            }
        );
    }

    /// 0xFFFF_FFFF * 2 = 0x1_FFFF_FFFE: v1 keeps the low word, v2 adds the high word (1).
    #[test]
    fn version_two_adds_the_high_half_of_each_product() {
        let mut checksum = Checksum::new(2, false, false);
        checksum.update(&[-1, -1, -1, -1]);
        // position 1: 0xFFFF_FFFF; position 2: 0x1_FFFF_FFFE.
        let v1 = 0xFFFF_FFFFu32.wrapping_add(0xFFFF_FFFE);
        assert_eq!(
            checksum.finish(),
            Sums {
                v1,
                v2: v1.wrapping_add(1)
            }
        );
    }

    #[test]
    fn first_and_last_track_edges_are_excluded() {
        let samples = vec![1i16; 2 * 10_000];
        let sum = |first, last| {
            let mut checksum = Checksum::new(10_000, first, last);
            checksum.update(&samples);
            checksum.finish().v1
        };
        let all = sum(false, false);
        // Word 0x0001_0001 times each position: removing positions 1..=2939 changes the sum.
        assert_ne!(sum(true, false), all);
        assert_ne!(sum(false, true), all);
        let expected_first: u32 =
            (2940..=10_000u32).fold(0, |s, p| s.wrapping_add(0x0001_0001u32.wrapping_mul(p)));
        assert_eq!(sum(true, false), expected_first);
        let expected_last: u32 =
            (1..=10_000 - 2940u32).fold(0, |s, p| s.wrapping_add(0x0001_0001u32.wrapping_mul(p)));
        assert_eq!(sum(false, true), expected_last);
    }

    fn toc_with(lead_out_lba: u32, starts_lba: &[u32]) -> Toc {
        Toc {
            first_track: 1,
            last_track: starts_lba.len() as u8,
            lead_out: lead_out_lba + LEAD_IN_SECTORS,
            offsets: starts_lba.iter().map(|lba| lba + LEAD_IN_SECTORS).collect(),
        }
    }

    /// CUETools' `OpenOneTrackCD` test expects the id "00041f6d-00083ece-020e1201". Its cue sheet is
    /// not available, but a one-track disc is fixed by those ids: id1 = start + lead-out and
    /// id2 = start + 2 * lead-out give a track starting at sector 12 and a lead-out at 270177, and
    /// the third (CDDB) component independently agrees (3602 s long, digit sum 2).
    #[test]
    fn one_track_disc_ids_match_the_cuetools_test() {
        let ids = disc_ids(&toc_with(270_177, &[12]));
        assert_eq!(
            (ids.id1, ids.id2, ids.cddb),
            (0x0004_1f6d, 0x0008_3ece, 0x020e_1201)
        );
    }

    #[test]
    fn first_track_at_zero_counts_as_one_in_id2() {
        let ids = disc_ids(&toc_with(1000, &[0, 400]));
        assert_eq!(ids.id1, 1400);
        assert_eq!(ids.id2, 1 + 400 * 2 + 1000 * 3);
    }

    #[test]
    fn database_url_shards_by_the_low_digits_of_id1() {
        // id1 is ...1f6d, so the directories are d, 6, f.
        let toc = toc_with(270_177, &[12]);
        assert_eq!(
            database_url(&toc),
            "https://www.accuraterip.com/accuraterip/d/6/f/dBAR-001-00041f6d-00083ece-020e1201.bin"
        );
    }

    fn record(ids: DiscIds, tracks: &[(u8, u32, u32)]) -> Vec<u8> {
        let mut bytes = vec![tracks.len() as u8];
        for id in [ids.id1, ids.id2, ids.cddb] {
            bytes.extend(id.to_le_bytes());
        }
        for &(confidence, crc, frame_450) in tracks {
            bytes.push(confidence);
            bytes.extend(crc.to_le_bytes());
            bytes.extend(frame_450.to_le_bytes());
        }
        bytes
    }

    const IDS: DiscIds = DiscIds {
        id1: 0x41f6d,
        id2: 0x83ece,
        cddb: 0x020e_1201,
    };

    #[test]
    fn parses_several_pressings() {
        let mut bytes = record(IDS, &[(12, 0xdead_beef, 1), (3, 0x0102_0304, 2)]);
        bytes.extend(record(DiscIds { id1: 9, ..IDS }, &[(1, 5, 6), (200, 7, 8)]));
        let pressings = parse_database(&bytes).unwrap();
        assert_eq!(pressings.len(), 2);
        assert_eq!(pressings[0].ids, IDS);
        assert_eq!(
            pressings[0].tracks[0],
            PressedTrack {
                confidence: 12,
                checksum: 0xdead_beef,
                frame_450: 1
            }
        );
        assert_eq!(pressings[0].tracks[1].checksum, 0x0102_0304);
        assert_eq!(pressings[1].ids.id1, 9);
        assert_eq!(pressings[1].tracks[1].confidence, 200);
    }

    #[test]
    fn empty_and_truncated_databases() {
        assert!(parse_database(&[]).unwrap().is_empty());
        let bytes = record(IDS, &[(1, 2, 3), (4, 5, 6)]);
        for cut in [5, 13, 14, bytes.len() - 1] {
            assert!(parse_database(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn verdicts() {
        let pressings = parse_database(
            &[
                record(IDS, &[(5, 100, 0), (7, 200, 0)]),
                record(IDS, &[(9, 100, 0), (1, 999, 0)]),
            ]
            .concat(),
        )
        .unwrap();
        let sums = |v1, v2| Sums { v1, v2 };
        assert_eq!(
            verdict(&pressings, 0, sums(100, 0)),
            AccurateRip::Matched { confidence: 9 }
        );
        // The version 2 checksum may be the one on file.
        assert_eq!(
            verdict(&pressings, 1, sums(0, 200)),
            AccurateRip::Matched { confidence: 7 }
        );
        assert_eq!(verdict(&pressings, 1, sums(1, 2)), AccurateRip::Mismatch);
        assert_eq!(
            verdict(&pressings, 2, sums(1, 2)),
            AccurateRip::NotInDatabase
        );
        assert_eq!(verdict(&[], 0, sums(100, 100)), AccurateRip::NotInDatabase);
    }
}
