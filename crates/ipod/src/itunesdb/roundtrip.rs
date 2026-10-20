//! The comparison behind [`crate::verify_roundtrip`]. It takes the database as a byte slice and
//! produces the rewrite as a new `Vec`, so it has no way to write to the device it came from.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::artwork;
use super::census::census;
use super::mhit::at;
use super::raw::{RawDb, RawMhod};
use super::{MHSD_TYPE_AT, database_chunk, emit, parse, sections};
use crate::bytes::u32_at;
use crate::error::Result;
use crate::verify::{
    ArtworkCheck, ChecksumCheck, ChunkCount, FieldDifference, HeaderChange, RecordDifference,
    RoundtripReport, SectionSize, describe_playlist, describe_track,
};

const REBUILT_SECTIONS: [u32; 5] = [1, 2, 3, 4, 8];
/// Header words the rewrite recomputes on purpose, per record kind.
const EXPECTED_MHBD: [usize; 1] = [8];
const EXPECTED_MHIT: [usize; 4] = [8, at::ALBUM_ID, at::ARTIST_ID, at::COMPOSER_ID];
const EXPECTED_MHYP: [usize; 3] = [8, 12, 16];

pub(crate) fn compare(
    original: &[u8],
    artwork_db: Option<&[u8]>,
    mount: &Path,
    firewire_id: Option<&[u8; 8]>,
) -> Result<RoundtripReport> {
    let raw_before = RawDb::parse(original)?;
    let rewritten = emit(&raw_before, None)?;
    let raw_after = RawDb::parse(&rewritten)?;
    let (tracks_before, playlists_before) = parse(original, mount)?;
    let (tracks_after, playlists_after) = parse(&rewritten, mount)?;

    let mut headers = HeaderTally::default();
    headers.add(
        "mhbd",
        &raw_before.header,
        &raw_after.header,
        &EXPECTED_MHBD,
    );
    let mut mhod_differences = Vec::new();

    for (before, after) in raw_before.tracks.iter().zip(&raw_after.tracks) {
        headers.add("mhit", &before.header, &after.header, &EXPECTED_MHIT);
        if mhods_differ(&before.mhods, &after.mhods) {
            mhod_differences.push(format!("track {}", before.id()));
        }
    }
    for (before, after) in raw_before.playlists.iter().zip(&raw_after.playlists) {
        headers.add("mhyp", &before.header, &after.header, &EXPECTED_MHYP);
        if mhods_differ(&before.mhods, &after.mhods) || before.members != after.members {
            mhod_differences.push(format!("playlist {}", before.id()));
        }
    }

    Ok(RoundtripReport {
        original_bytes: original.len() as u64,
        rewritten_bytes: rewritten.len() as u64,
        tracks: tracks_before.len() as u64,
        playlists: playlists_before.len() as u64,
        checksum: checksum_check(original, firewire_id),
        artwork: artwork_check(artwork_db, mount),
        sections: section_sizes(original, &rewritten)?,
        header_changes: headers.finish(),
        track_differences: differences(&tracks_before, &tracks_after, describe_track),
        playlist_differences: differences(&playlists_before, &playlists_after, describe_playlist),
        mhod_differences,
        chunk_counts: chunk_counts(original, &rewritten)?,
    })
}

fn artwork_check(artwork_db: Option<&[u8]>, mount: &Path) -> ArtworkCheck {
    let Some(bytes) = artwork_db else {
        return ArtworkCheck::Absent;
    };
    match artwork::check(mount, bytes) {
        Ok(check) => ArtworkCheck::Checked {
            images: check.images,
            thumbnails: check.thumbnails,
            missing_thumbnails: check.missing,
            identical: check.identical,
        },
        Err(error) => ArtworkCheck::Unreadable(error.to_string()),
    }
}

fn checksum_check(file: &[u8], firewire_id: Option<&[u8; 8]>) -> ChecksumCheck {
    match (super::checksum_scheme(file), firewire_id) {
        (None | Some(0), _) => ChecksumCheck::Absent,
        (Some(1), None) => ChecksumCheck::NoFirewireGuid,
        (Some(1), Some(id)) => match super::verify_checksum(file, id) {
            Some(true) => ChecksumCheck::Verified,
            _ => ChecksumCheck::Mismatch,
        },
        (Some(other), _) => ChecksumCheck::OtherScheme(other),
    }
}

fn section_sizes(original: &[u8], rewritten: &[u8]) -> Result<Vec<SectionSize>> {
    let lengths = |data: &[u8]| -> Result<Vec<(u32, u64)>> {
        let database = database_chunk(data)?;
        Ok(sections(data, &database)?
            .iter()
            .map(|section| {
                (
                    section.u32(MHSD_TYPE_AT).unwrap_or(0),
                    (section.end - section.offset) as u64,
                )
            })
            .collect())
    };
    Ok(lengths(original)?
        .into_iter()
        .zip(lengths(rewritten)?)
        .map(
            |((kind, original_bytes), (_, rewritten_bytes))| SectionSize {
                kind,
                original_bytes,
                rewritten_bytes,
                rebuilt: REBUILT_SECTIONS.contains(&kind),
            },
        )
        .collect())
}

fn chunk_counts(original: &[u8], rewritten: &[u8]) -> Result<Vec<ChunkCount>> {
    let (before, after) = (census(original)?, census(rewritten)?);
    let kinds: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    Ok(kinds
        .into_iter()
        .map(|kind| ChunkCount {
            chunk: kind.clone(),
            original: before.get(kind).copied().unwrap_or(0),
            rewritten: after.get(kind).copied().unwrap_or(0),
        })
        .filter(|count| count.original != count.rewritten)
        .collect())
}

fn mhods_differ(before: &[RawMhod], after: &[RawMhod]) -> bool {
    before.len() != after.len()
        || before
            .iter()
            .zip(after)
            .any(|(a, b)| a.kind != b.kind || a.bytes != b.bytes)
}

/// Field-by-field differences of the reader's view, using the serialised form so every field the
/// public types carry is compared without listing them here.
fn differences<T: Serialize>(
    before: &[T],
    after: &[T],
    describe: impl Fn(&T) -> (String, String),
) -> Vec<RecordDifference> {
    before
        .iter()
        .zip(after)
        .filter_map(|(a, b)| {
            let (Ok(Value::Object(a_fields)), Ok(Value::Object(b_fields))) =
                (serde_json::to_value(a), serde_json::to_value(b))
            else {
                return None;
            };
            let fields: Vec<FieldDifference> = a_fields
                .iter()
                .filter(|(key, value)| b_fields.get(*key) != Some(*value))
                .map(|(key, value)| FieldDifference {
                    field: key.clone(),
                    before: value.to_string(),
                    after: b_fields
                        .get(key)
                        .map_or_else(|| "missing".to_owned(), Value::to_string),
                })
                .collect();
            let (record, name) = describe(a);
            (!fields.is_empty()).then_some(RecordDifference {
                record,
                name,
                fields,
            })
        })
        .collect()
}

/// Header words that changed, summed over all records of a kind.
#[derive(Default)]
struct HeaderTally(BTreeMap<(String, usize), HeaderChange>);

impl HeaderTally {
    fn add(&mut self, record: &str, before: &[u8], after: &[u8], expected: &[usize]) {
        for offset in (0..before.len().max(after.len())).step_by(4) {
            let (a, b) = (u32_at(before, offset), u32_at(after, offset));
            if a == b {
                continue;
            }
            let change = self
                .0
                .entry((record.to_owned(), offset))
                .or_insert_with(|| HeaderChange {
                    record: record.to_owned(),
                    offset,
                    records_changed: 0,
                    example_before: a.unwrap_or(0),
                    example_after: b.unwrap_or(0),
                    expected: expected.contains(&offset),
                });
            change.records_changed += 1;
        }
    }

    fn finish(self) -> Vec<HeaderChange> {
        self.0.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::IpodTrack;

    #[test]
    fn differing_fields_are_listed_by_name_with_both_values() {
        let before = IpodTrack {
            id: 3,
            title: "Old".into(),
            rating: 40,
            ..IpodTrack::default()
        };
        let after = IpodTrack {
            id: 3,
            title: "New".into(),
            rating: 40,
            ..IpodTrack::default()
        };

        let found = differences(&[before], &[after], describe_track);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].record, "track 3");
        assert_eq!(found[0].name, "Old");
        assert_eq!(
            found[0].fields,
            [FieldDifference {
                field: "title".into(),
                before: "\"Old\"".into(),
                after: "\"New\"".into()
            }]
        );
    }

    #[test]
    fn identical_records_have_no_differences() {
        let track = IpodTrack {
            title: "Same".into(),
            ..IpodTrack::default()
        };

        assert!(
            differences(
                std::slice::from_ref(&track),
                std::slice::from_ref(&track),
                describe_track
            )
            .is_empty()
        );
    }

    #[test]
    fn header_changes_are_summed_per_word_and_flagged_when_expected() {
        let mut tally = HeaderTally::default();
        tally.add(
            "mhit",
            &[1, 0, 0, 0, 5, 0, 0, 0],
            &[2, 0, 0, 0, 6, 0, 0, 0],
            &[0],
        );
        tally.add(
            "mhit",
            &[1, 0, 0, 0, 5, 0, 0, 0],
            &[3, 0, 0, 0, 5, 0, 0, 0],
            &[0],
        );

        let changes = tally.finish();

        assert_eq!(changes.len(), 2);
        assert_eq!(
            (
                changes[0].offset,
                changes[0].records_changed,
                changes[0].expected
            ),
            (0, 2, true)
        );
        assert_eq!(
            (
                changes[1].offset,
                changes[1].records_changed,
                changes[1].expected
            ),
            (4, 1, false)
        );
        assert_eq!(
            (changes[1].example_before, changes[1].example_after),
            (5, 6)
        );
    }
}
