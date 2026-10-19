//! Parsing `.TOC.plist`, the table of contents macOS's cddafs writes at the root of a mounted
//! audio CD.
//!
//! The format comes from Apple's open-source cddafs (`FilesystemProber/cddafs_util.cpp`,
//! `CreateXMLFileInPListFormat`): a top-level dictionary with the raw TOC under
//! `Format 0x02 TOC Data` and a `Sessions` array. Each session is a dictionary of
//! `Session Number`, `Session Type`, `First Track`, `Last Track`, `Leadout Block` and a
//! `Track Array` whose dictionaries hold `Point` (the track number), `Start Block`,
//! `Session Number`, `Data` (true for data tracks) and `Pre-Emphasis Enabled`.
//!
//! cddafs computes `Start Block` and `Leadout Block` as `(minutes * 60 + seconds) * 75 + frames`
//! from the absolute-time fields of the raw TOC, so they include the 150 sector lead-in, which is
//! how MusicBrainz counts. Should a macOS version ever write plain LBAs instead (track 1 at 0), the
//! first track would start before sector 150, which is impossible on a real disc, so such a TOC is
//! shifted by the lead-in.

use std::path::Path;

use plist::{Dictionary, Value};

use crate::{
    Error, Result,
    toc::{LEAD_IN_SECTORS, Toc},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedToc {
    pub toc: Toc,
    /// Whether each track in the TOC is a data track, in track order.
    pub is_data: Vec<bool>,
}

pub(crate) fn read(path: &Path) -> Result<ParsedToc> {
    let value =
        Value::from_file(path).map_err(|err| Error::Disc(format!("{}: {err}", path.display())))?;
    parse(&value)
}

fn parse(value: &Value) -> Result<ParsedToc> {
    let missing = |what: &str| Error::Disc(format!("TOC plist has no {what}"));
    let session = value
        .as_dictionary()
        .and_then(|root| root.get("Sessions"))
        .and_then(Value::as_array)
        .and_then(|sessions| sessions.first())
        .and_then(Value::as_dictionary)
        .ok_or_else(|| missing("session"))?;

    let tracks: Vec<&Dictionary> = session
        .get("Track Array")
        .and_then(Value::as_array)
        .ok_or_else(|| missing("track array"))?
        .iter()
        .filter_map(Value::as_dictionary)
        .collect();

    let mut entries = tracks
        .iter()
        .map(|track| {
            Ok((
                number(track, "Point").ok_or_else(|| missing("track number"))?,
                number(track, "Start Block").ok_or_else(|| missing("track start"))?,
                track
                    .get("Data")
                    .and_then(Value::as_boolean)
                    .unwrap_or(false),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by_key(|&(point, ..)| point);
    let (&(first, ..), &(last, ..)) = entries
        .first()
        .zip(entries.last())
        .ok_or_else(|| missing("tracks"))?;
    let first_track = u8::try_from(number(session, "First Track").unwrap_or(first))
        .map_err(|_| missing("valid first track"))?;
    let last_track = u8::try_from(number(session, "Last Track").unwrap_or(last))
        .map_err(|_| missing("valid last track"))?;
    if entries.len() != usize::from(last_track.saturating_sub(first_track)) + 1 {
        return Err(Error::Disc(format!(
            "TOC lists {} tracks for tracks {first_track} to {last_track}",
            entries.len()
        )));
    }

    let lead_out = number(session, "Leadout Block").ok_or_else(|| missing("lead-out"))?;
    // A real first track begins after the 150 sector lead-in; anything earlier means plain LBAs.
    let shift = if entries
        .iter()
        .any(|&(_, start, _)| start < LEAD_IN_SECTORS.into())
    {
        LEAD_IN_SECTORS.into()
    } else {
        0
    };
    let sector = |value: i64| {
        u32::try_from(value + shift).map_err(|_| Error::Disc("TOC sector out of range".into()))
    };

    let offsets = entries
        .iter()
        .map(|&(_, start, _)| sector(start))
        .collect::<Result<Vec<_>>>()?;
    let lead_out = sector(lead_out)?;
    if !offsets.windows(2).all(|pair| pair[0] < pair[1])
        || offsets.last().is_none_or(|&last| last >= lead_out)
    {
        return Err(Error::Disc("TOC track starts are not in order".into()));
    }
    Ok(ParsedToc {
        toc: Toc {
            first_track,
            last_track,
            lead_out,
            offsets,
        },
        is_data: entries.iter().map(|&(.., data)| data).collect(),
    })
}

fn number(dict: &Dictionary, key: &str) -> Option<i64> {
    let value = dict.get(key)?;
    value.as_signed_integer().or_else(|| {
        value
            .as_unsigned_integer()
            .and_then(|n| i64::try_from(n).ok())
    })
}

#[cfg(test)]
pub(crate) fn xml(first: u8, lead_out: u32, starts: &[u32], data: &[bool]) -> String {
    let tracks: String = starts
        .iter()
        .zip(first..)
        .enumerate()
        .map(|(index, (start, point))| {
            format!(
                "<dict><key>Data</key><{}/><key>Point</key><integer>{point}</integer>\
                 <key>Pre-Emphasis Enabled</key><false/><key>Session Number</key><integer>1</integer>\
                 <key>Start Block</key><integer>{start}</integer></dict>",
                if data.get(index).copied().unwrap_or(false) { "true" } else { "false" }
            )
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\
         <key>Format 0x02 TOC Data</key><data>AAA=</data>\
         <key>Sessions</key><array><dict><key>First Track</key><integer>{first}</integer>\
         <key>Last Track</key><integer>{last}</integer><key>Leadout Block</key><integer>{lead_out}</integer>\
         <key>Session Number</key><integer>1</integer><key>Session Type</key><integer>0</integer>\
         <key>Track Array</key><array>{tracks}</array></dict></array></dict></plist>",
        last = usize::from(first) + starts.len() - 1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        musicbrainz_disc_id,
        toc::vectors::{ALL, GUANO_APES},
    };

    fn parse_xml(text: &str) -> Result<ParsedToc> {
        parse(&Value::from_reader_xml(text.as_bytes()).unwrap())
    }

    #[test]
    fn absolute_blocks_are_taken_as_they_are() {
        let guano = &ALL[0];
        let parsed = parse_xml(&xml(1, 258_725, &GUANO_APES, &[])).unwrap();
        assert_eq!(parsed.toc, (guano.toc)());
        assert_eq!(musicbrainz_disc_id(&parsed.toc), guano.musicbrainz);
        assert!(parsed.is_data.iter().all(|&data| !data));
    }

    #[test]
    fn plain_lba_blocks_are_shifted_by_the_lead_in() {
        let lba: Vec<u32> = GUANO_APES.iter().map(|sector| sector - 150).collect();
        let parsed = parse_xml(&xml(1, 258_725 - 150, &lba, &[])).unwrap();
        assert_eq!(musicbrainz_disc_id(&parsed.toc), ALL[0].musicbrainz);
    }

    #[test]
    fn a_first_track_other_than_one_is_kept() {
        let lunar = &ALL[1];
        let toc = (lunar.toc)();
        let parsed = parse_xml(&xml(2, toc.lead_out, &toc.offsets, &[])).unwrap();
        assert_eq!((parsed.toc.first_track, parsed.toc.last_track), (2, 11));
        assert_eq!(musicbrainz_disc_id(&parsed.toc), lunar.musicbrainz);
    }

    #[test]
    fn data_tracks_are_flagged() {
        let parsed = parse_xml(&xml(
            1,
            100_000,
            &[150, 20_000, 60_000],
            &[false, false, true],
        ))
        .unwrap();
        assert_eq!(parsed.is_data, [false, false, true]);
    }

    #[test]
    fn tracks_are_ordered_by_point_whatever_the_array_order() {
        let text = xml(1, 100_000, &[150, 20_000, 60_000], &[]);
        // Swap the first two track dictionaries.
        let first = text.find("<dict><key>Data").unwrap();
        let second = text[first + 1..].find("<dict><key>Data").unwrap() + first + 1;
        let third = text[second + 1..].find("<dict><key>Data").unwrap() + second + 1;
        let swapped = format!(
            "{}{}{}{}",
            &text[..first],
            &text[second..third],
            &text[first..second],
            &text[third..]
        );
        let parsed = parse_xml(&swapped).unwrap();
        assert_eq!(parsed.toc.offsets, [150, 20_000, 60_000]);
    }

    #[test]
    fn malformed_tocs_are_errors_not_panics() {
        for bad in [
            "<plist version=\"1.0\"><dict/></plist>",
            "<plist version=\"1.0\"><dict><key>Sessions</key><array/></dict></plist>",
            "<plist version=\"1.0\"><array/></plist>",
        ] {
            assert!(matches!(parse_xml(bad), Err(Error::Disc(_))), "{bad}");
        }
        // Lead-out before the last track.
        assert!(parse_xml(&xml(1, 100, &[150, 20_000], &[])).is_err());
        // Starts out of order.
        assert!(parse_xml(&xml(1, 100_000, &[20_000, 150], &[])).is_err());
    }

    #[test]
    fn reads_binary_plists_too() {
        let value = Value::from_reader_xml(xml(1, 258_725, &GUANO_APES, &[]).as_bytes()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".TOC.plist");
        value.to_file_binary(&path).unwrap();
        assert_eq!(read(&path).unwrap().toc, (ALL[0].toc)());
    }
}
