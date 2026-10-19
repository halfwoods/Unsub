//! A disc's table of contents and the identifiers derived from it.

use base64::{Engine, engine::general_purpose::STANDARD};
use sha1::{Digest, Sha1};

/// Every sector offset in a CD TOC includes the two-second (150 sector) lead-in, as MusicBrainz,
/// FreeDB and the CD standard itself count them.
pub const LEAD_IN_SECTORS: u32 = 150;
pub const SECTORS_PER_SECOND: u32 = 75;
/// 1/75 s of 44.1 kHz audio.
pub const FRAMES_PER_SECTOR: u32 = 588;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toc {
    pub first_track: u8,
    pub last_track: u8,
    /// Sector where the lead-out starts.
    pub lead_out: u32,
    /// Start sector of each track from `first_track` to `last_track`.
    pub offsets: Vec<u32>,
}

impl Toc {
    pub fn track_count(&self) -> usize {
        self.offsets.len()
    }

    /// First track starts, lead-out last.
    pub(crate) fn boundaries(&self) -> impl Iterator<Item = u32> + '_ {
        self.offsets.iter().copied().chain([self.lead_out])
    }
}

/// The MusicBrainz DiscID: SHA-1 over the first and last track numbers and 100 offsets (the
/// lead-out, then tracks 1 to 99, unused slots zero), as upper-case hex, in MusicBrainz's base64
/// variant (`+` `/` `=` become `.` `_` `-`).
pub fn musicbrainz_disc_id(toc: &Toc) -> String {
    let mut slots = [0u32; 100];
    slots[0] = toc.lead_out;
    for (track, offset) in (toc.first_track..=toc.last_track).zip(&toc.offsets) {
        if let Some(slot) = slots.get_mut(usize::from(track)) {
            *slot = *offset;
        }
    }
    let mut text = format!("{:02X}{:02X}", toc.first_track, toc.last_track);
    for slot in slots {
        text.push_str(&format!("{slot:08X}"));
    }
    STANDARD
        .encode(Sha1::digest(text.as_bytes()))
        .replace('+', ".")
        .replace('/', "_")
        .replace('=', "-")
}

/// The classic FreeDB / CDDB id, also the third component of an AccurateRip id. FreeDB only knows
/// discs whose first track is 1, so it counts tracks rather than reading `last_track`.
pub fn cddb_disc_id(toc: &Toc) -> u32 {
    let digit_sum = |mut n: u32| {
        let mut sum = 0;
        while n > 0 {
            sum += n % 10;
            n /= 10;
        }
        sum
    };
    let checksum: u32 = toc
        .offsets
        .iter()
        .map(|&offset| digit_sum(offset / SECTORS_PER_SECOND))
        .sum();
    let first = toc.offsets.first().copied().unwrap_or(0);
    let seconds = (toc.lead_out / SECTORS_PER_SECOND).saturating_sub(first / SECTORS_PER_SECOND);
    ((checksum % 255) << 24) | (seconds << 8) | toc.track_count() as u32
}

#[cfg(test)]
pub(crate) mod vectors {
    //! The test discs from python-discid's own suite (metabrainz/python-discid, test_discid.py).
    use super::Toc;

    pub struct Vector {
        pub name: &'static str,
        pub toc: fn() -> Toc,
        pub musicbrainz: &'static str,
        /// None where libdiscid's answer is a quirk: with a first track other than 1 it measures the
        /// length from a track 1 that does not exist, which FreeDB itself cannot represent.
        pub freedb: Option<u32>,
    }

    pub const GUANO_APES: [u32; 15] = [
        150, 17510, 33275, 45910, 57805, 78310, 94650, 109580, 132010, 149160, 165115, 177710,
        203325, 215555, 235590,
    ];

    pub const ALL: [Vector; 4] = [
        Vector {
            name: "Guano Apes - Don't Give Me Names",
            toc: || Toc {
                first_track: 1,
                last_track: 15,
                lead_out: 258_725,
                offsets: GUANO_APES.to_vec(),
            },
            musicbrainz: "TqvKjMu7dMliSfmVEBtrL7sBSno-",
            freedb: Some(0xb60d_770f),
        },
        Vector {
            name: "Lunar - There Is No 1 (first track is 2)",
            toc: || Toc {
                first_track: 2,
                last_track: 11,
                lead_out: 225_781,
                offsets: vec![
                    150, 11512, 34143, 50747, 63640, 98491, 123534, 174410, 195438, 201127,
                ],
            },
            musicbrainz: "6RDuz0d7.M5SVMLe1z4DP0yaEC8-",
            freedb: None,
        },
        Vector {
            name: "Minimal",
            toc: || Toc {
                first_track: 1,
                last_track: 1,
                lead_out: 44_942,
                offsets: vec![150],
            },
            musicbrainz: "ANJa4DGYN_ktpzOwvVPtcjwP7mE-",
            freedb: Some(0x0202_5501),
        },
        Vector {
            name: "Korn - See You on the Other Side (pre-gap audio)",
            toc: || Toc {
                first_track: 1,
                last_track: 14,
                lead_out: 275_749,
                offsets: vec![
                    5475, 19645, 34416, 51655, 68900, 90015, 111090, 130510, 158652, 173635,
                    189015, 208122, 224413, 252866,
                ],
            },
            musicbrainz: "CnkXRItZOUxex7JwyWmHfdbFdqE-",
            freedb: Some(0xbe0e_130e),
        },
    ];
}

#[cfg(test)]
mod tests {
    use super::{vectors::ALL, *};

    #[test]
    fn musicbrainz_ids_match_python_discids_test_discs() {
        for vector in &ALL {
            assert_eq!(
                musicbrainz_disc_id(&(vector.toc)()),
                vector.musicbrainz,
                "{}",
                vector.name
            );
        }
    }

    #[test]
    fn freedb_ids_match_python_discids_test_discs() {
        for vector in &ALL {
            if let Some(freedb) = vector.freedb {
                assert_eq!(cddb_disc_id(&(vector.toc)()), freedb, "{}", vector.name);
            }
        }
    }

    #[test]
    fn disc_ids_are_28_characters_of_the_musicbrainz_alphabet() {
        for vector in &ALL {
            let id = musicbrainz_disc_id(&(vector.toc)());
            assert_eq!(id.len(), 28);
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            );
            assert!(id.ends_with('-'));
        }
    }
}
