use std::path::PathBuf;

use lofty::file::{TaggedFile, TaggedFileExt};
use lofty::tag::ItemKey;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::RADIO;

/// ReplayGain 2.0 aims at -18 LUFS: a track measuring -18 LUFS has a gain of 0 dB.
const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;
/// EBU R128 (and Opus's `R128_*_GAIN` tags) aim at -23 LUFS.
const R128_REFERENCE_LUFS: f64 = -23.0;
/// Opus stores R128 gains as signed Q7.8 fixed point: units of 1/256 dB.
const R128_UNITS_PER_DB: f64 = 256.0;
/// iTunNORM's first words are `1000 * 10^(-gain/10)`.
const ITUNNORM_UNITY: f64 = 1000.0;
/// iTunNORM's peak words are sample values, 32768 being full scale for 16 bits.
const ITUNNORM_FULL_SCALE: f64 = 32768.0;
/// Tags are untrusted: a gain beyond this is a corrupt tag, not a very quiet song.
const MAX_ABS_GAIN_DB: f64 = 40.0;
/// A linear peak beyond this (+18 dB over full scale) is not credible either.
const MAX_LINEAR_PEAK: f64 = 8.0;
/// Bounds for values a caller reports from its own measurement.
const LUFS_RANGE: std::ops::RangeInclusive<f64> = -90.0..=20.0;
const PEAK_DBTP_RANGE: std::ops::RangeInclusive<f64> = -120.0..=30.0;

/// Where a track's loudness came from, which says how far to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoudnessSource {
    /// Measured by this app (EBU R128).
    #[serde(rename = "analyzed")]
    Analyzed,
    #[serde(rename = "replaygain")]
    ReplayGain,
    /// iTunes' Sound Check data, read as if it were ReplayGain.
    #[serde(rename = "itunnorm")]
    ITunNorm,
    #[serde(rename = "r128tag")]
    R128Tag,
}

impl LoudnessSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Analyzed => "analyzed",
            Self::ReplayGain => "replaygain",
            Self::ITunNorm => "itunnorm",
            Self::R128Tag => "r128tag",
        }
    }
}

impl ToSql for LoudnessSource {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for LoudnessSource {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "analyzed" => Ok(Self::Analyzed),
            "replaygain" => Ok(Self::ReplayGain),
            "itunnorm" => Ok(Self::ITunNorm),
            "r128tag" => Ok(Self::R128Tag),
            other => Err(FromSqlError::Other(
                format!("unknown loudness source {other:?}").into(),
            )),
        }
    }
}

/// How loud a track is, on the LUFS scale whatever tag it was read from.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackLoudness {
    /// Integrated loudness.
    pub lufs: f64,
    /// Highest peak, in dB relative to full scale (true peak if measured here,
    /// sample peak when a tag gave it). Not every tag carries one.
    pub true_peak_dbtp: Option<f64>,
    pub source: LoudnessSource,
    /// The album's loudness, when the file's own tags state it.
    pub album_lufs: Option<f64>,
    pub album_true_peak_dbtp: Option<f64>,
}

/// A measurement by the app's own analyzer.
#[derive(Debug, Clone, PartialEq)]
pub struct Loudness {
    pub lufs: f64,
    pub true_peak_dbtp: f64,
    /// Whatever the analyzer needs to combine this track into an album
    /// measurement later; the library stores it without looking inside.
    pub state: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumLoudness {
    pub lufs: f64,
    pub true_peak_dbtp: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GainMode {
    Track,
    Album,
}

/// What to apply to bring a track to the target loudness.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gain {
    pub db: f64,
    /// The peak before the gain is applied, so a player can tell whether the
    /// gain would clip.
    pub peak_dbtp: Option<f64>,
}

/// The iTunesDB `soundcheck` field for a gain in dB (positive turns a track up).
/// libgpod documents it: "X = 1000 * 10 ^ (-.1 * Y) where Y is the adjustment
/// value in dB and X is the value that goes into the SoundCheck field. The
/// value 0 is special [...] treated as 'no Soundcheck'", and notes that the
/// equation works for ReplayGain-derived data (`itdb_track::soundcheck` in
/// libgpod's `itdb.h`). The same 1000-based scale is iTunNORM's first word.
/// Never returns 0, since that would switch Sound Check off for the track.
pub fn itunes_soundcheck(gain_db: f64) -> u32 {
    if !gain_db.is_finite() {
        return 1000;
    }
    (ITUNNORM_UNITY * 10f64.powf(-gain_db / 10.0))
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Reads whatever loudness data the file carries, preferring ReplayGain, then
/// Opus R128, then iTunes' Sound Check. `itunnorm` is the text of the
/// `iTunNORM` comment, which the generic tag API does not expose.
pub(crate) fn from_tags(file: &TaggedFile, itunnorm: Option<&str>) -> Option<TrackLoudness> {
    let text = |key: ItemKey| file.tags().iter().find_map(|tag| tag.get_string(key));

    let album_from_replaygain = text(ItemKey::ReplayGainAlbumGain)
        .and_then(replaygain_to_lufs)
        .map(|lufs| {
            (
                lufs,
                text(ItemKey::ReplayGainAlbumPeak).and_then(peak_to_dbtp),
            )
        });
    let album_from_r128 = text(ItemKey::R128AlbumGain)
        .and_then(r128_to_lufs)
        .map(|lufs| (lufs, None));
    let album = album_from_replaygain.or(album_from_r128);

    let (lufs, peak, source) =
        if let Some(lufs) = text(ItemKey::ReplayGainTrackGain).and_then(replaygain_to_lufs) {
            let peak = text(ItemKey::ReplayGainTrackPeak).and_then(peak_to_dbtp);
            (lufs, peak, LoudnessSource::ReplayGain)
        } else if let Some(lufs) = text(ItemKey::R128TrackGain).and_then(r128_to_lufs) {
            (lufs, None, LoudnessSource::R128Tag)
        } else {
            let (lufs, peak) = itunnorm.and_then(parse_itunnorm)?;
            (lufs, peak, LoudnessSource::ITunNorm)
        };
    Some(TrackLoudness {
        lufs,
        true_peak_dbtp: peak,
        source,
        album_lufs: album.map(|(lufs, _)| lufs),
        album_true_peak_dbtp: album.and_then(|(_, peak)| peak),
    })
}

/// "-6.50 dB", "+3.2", "0.00 db". The gain is relative to -18 LUFS.
fn replaygain_to_lufs(text: &str) -> Option<f64> {
    let number = text
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .trim();
    let gain: f64 = number.parse().ok()?;
    gain_to_lufs(gain, REPLAYGAIN_REFERENCE_LUFS)
}

/// A signed integer in 1/256 dB, relative to -23 LUFS.
fn r128_to_lufs(text: &str) -> Option<f64> {
    let units: i32 = text.trim().parse().ok()?;
    gain_to_lufs(f64::from(units) / R128_UNITS_PER_DB, R128_REFERENCE_LUFS)
}

fn gain_to_lufs(gain_db: f64, reference: f64) -> Option<f64> {
    (gain_db.is_finite() && gain_db.abs() <= MAX_ABS_GAIN_DB).then_some(reference - gain_db)
}

/// A linear peak ("0.988553", 1.0 being full scale) as dB; none if it is not a
/// believable positive number.
fn peak_to_dbtp(text: &str) -> Option<f64> {
    linear_peak_to_dbtp(text.trim().parse().ok()?)
}

fn linear_peak_to_dbtp(peak: f64) -> Option<f64> {
    (peak.is_finite() && peak > 0.0 && peak <= MAX_LINEAR_PEAK).then(|| 20.0 * peak.log10())
}

/// The `iTunNORM` comment: ten 8-digit hex words, a space before each, per
/// <https://id3.org/iTunes%20Normalization%20settings> (itself quoting Slim
/// Devices' `SoundCheck.pm`). Words 0 and 1 are the left and right volume
/// adjustment as `1000 * 10^(-gain/10)`; iTunes applies the larger, so that is
/// the one read. Words 6 and 7 are the peak sample value. The rest are unknown.
/// The gain is read as ReplayGain-equivalent, as libgpod and Slim Devices do.
fn parse_itunnorm(text: &str) -> Option<(f64, Option<f64>)> {
    let words: Vec<u64> = text
        .split_whitespace()
        .map(|word| u64::from_str_radix(word, 16))
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    if words.len() != 10 {
        return None;
    }
    let adjustment = words[0].max(words[1]);
    if adjustment == 0 {
        return None;
    }
    let gain_db = -10.0 * (adjustment as f64 / ITUNNORM_UNITY).log10();
    let lufs = gain_to_lufs(gain_db, REPLAYGAIN_REFERENCE_LUFS)?;
    let peak = linear_peak_to_dbtp(words[6].max(words[7]) as f64 / ITUNNORM_FULL_SCALE);
    Some((lufs, peak))
}

impl Library {
    /// Records the app's own measurement of a track, replacing whatever the
    /// tags said. A later edit of the track's tags leaves it alone, and so does a
    /// re-read of an unchanged file.
    pub fn set_loudness(&mut self, id: i64, loudness: &Loudness) -> Result<()> {
        if !LUFS_RANGE.contains(&loudness.lufs)
            || !PEAK_DBTP_RANGE.contains(&loudness.true_peak_dbtp)
        {
            return Err(Error::InvalidLoudness(format!(
                "{} LUFS with a {} dBTP peak",
                loudness.lufs, loudness.true_peak_dbtp
            )));
        }
        let changed = self.conn.execute(
            "UPDATE tracks SET loudness_lufs = ?2, true_peak_dbtp = ?3, loudness_source = ?4,
                               loudness_state = ?5
             WHERE id = ?1",
            params![
                id,
                loudness.lufs,
                loudness.true_peak_dbtp,
                LoudnessSource::Analyzed,
                loudness.state
            ],
        )?;
        if changed == 0 {
            return Err(Error::TrackNotFound(id));
        }
        Ok(())
    }

    /// The analyzer's stored state for a track, if it left any.
    pub fn loudness_state(&self, id: i64) -> Result<Option<Vec<u8>>> {
        let state: Option<Option<Vec<u8>>> = self
            .conn
            .query_row(
                "SELECT loudness_state FROM tracks WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(state.flatten())
    }

    /// Tracks the analyzer should measure: files that are here, not
    /// placeholders, and have no loudness yet, whatever their kind (but not
    /// stations, which have no file).
    pub fn tracks_needing_loudness(&self, limit: usize) -> Result<Vec<(i64, PathBuf)>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT id, path FROM tracks
             WHERE loudness_lufs IS NULL AND missing = 0 AND in_cloud = 0 AND kind <> {RADIO}
             ORDER BY id LIMIT ?1"
        ))?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The loudness of an album (by `AlbumSummary::key`). Albums whose files
    /// state it in their tags use that; otherwise it is combined from the
    /// tracks' own loudness, weighted by length, and only if every track has
    /// some. Playing an album as a whole needs all of its tracks measured.
    pub fn album_gain(&self, album_key: &str) -> Result<Option<AlbumLoudness>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT loudness_lufs, true_peak_dbtp, album_loudness_lufs, album_true_peak_dbtp,
                    duration_ms
             FROM tracks WHERE album_group = ?1",
        )?;
        let rows: Vec<AlbumRow> = stmt
            .query_map([album_key], |row| {
                Ok(AlbumRow {
                    lufs: row.get(0)?,
                    peak: row.get(1)?,
                    album_lufs: row.get(2)?,
                    album_peak: row.get(3)?,
                    duration_ms: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(combine_album(&rows))
    }

    /// The gain that brings a track (or its album) to `target_lufs`. In album
    /// mode a track whose album has no loudness falls back to its own. None
    /// when nothing is known about the track yet.
    pub fn sound_check_gain(
        &self,
        id: i64,
        mode: GainMode,
        target_lufs: f64,
    ) -> Result<Option<Gain>> {
        if !target_lufs.is_finite() {
            return Ok(None);
        }
        let row: Option<(Option<f64>, Option<f64>, String)> = self
            .conn
            .query_row(
                "SELECT loudness_lufs, true_peak_dbtp, album_group FROM tracks WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((lufs, peak, album_group)) = row else {
            return Ok(None);
        };
        let album = match mode {
            GainMode::Album => self.album_gain(&album_group)?,
            GainMode::Track => None,
        };
        let (lufs, peak) = match (album, lufs) {
            (Some(album), _) => (album.lufs, album.true_peak_dbtp),
            (None, Some(lufs)) => (lufs, peak),
            (None, None) => return Ok(None),
        };
        Ok(Some(Gain {
            db: target_lufs - lufs,
            peak_dbtp: peak,
        }))
    }
}

struct AlbumRow {
    lufs: Option<f64>,
    peak: Option<f64>,
    album_lufs: Option<f64>,
    album_peak: Option<f64>,
    duration_ms: u64,
}

fn combine_album(rows: &[AlbumRow]) -> Option<AlbumLoudness> {
    let mut stated: Vec<f64> = rows.iter().filter_map(|row| row.album_lufs).collect();
    if !stated.is_empty() {
        // Tracks of one album normally agree; the middle value shrugs off an odd one.
        stated.sort_by(f64::total_cmp);
        return Some(AlbumLoudness {
            lufs: stated[(stated.len() - 1) / 2],
            true_peak_dbtp: rows
                .iter()
                .filter_map(|row| row.album_peak)
                .max_by(f64::total_cmp),
        });
    }

    let measured: Vec<(f64, f64)> = rows
        .iter()
        .map(|row| row.lufs.map(|lufs| (lufs, row.duration_ms.max(1) as f64)))
        .collect::<Option<_>>()?;
    if measured.is_empty() {
        return None;
    }
    // Energy averaged over time: how loud the album would measure played end to end.
    let energy: f64 = measured
        .iter()
        .map(|(lufs, ms)| ms * 10f64.powf(lufs / 10.0))
        .sum();
    let total: f64 = measured.iter().map(|(_, ms)| ms).sum();
    let peaks: Option<Vec<f64>> = rows.iter().map(|row| row.peak).collect();
    Some(AlbumLoudness {
        lufs: 10.0 * (energy / total).log10(),
        true_peak_dbtp: peaks.and_then(|peaks| peaks.into_iter().max_by(f64::total_cmp)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 0.01, "{actual} vs {expected}");
    }

    #[test]
    fn replaygain_gains_are_relative_to_minus_18_lufs() {
        for (text, lufs) in [
            ("-6.50 dB", -11.5),
            ("+3.00 dB", -21.0),
            ("0.00 dB", -18.0),
            ("-6.5", -11.5),
            ("  -6.50 db ", -11.5),
            ("2.35DB", -20.35),
            ("-40.00 dB", 22.0),
            ("40 dB", -58.0),
        ] {
            close(
                replaygain_to_lufs(text).unwrap_or_else(|| panic!("{text:?}")),
                lufs,
            );
        }
    }

    #[test]
    fn absurd_or_unreadable_gains_are_refused() {
        for text in [
            "", "loud", "NaN dB", "inf", "-inf dB", "40.01 dB", "-41 dB", "1e9", "6 dB 7", "--3",
        ] {
            assert_eq!(replaygain_to_lufs(text), None, "{text:?}");
        }
    }

    #[test]
    fn r128_gains_are_q7_8_relative_to_minus_23_lufs() {
        close(r128_to_lufs("0").unwrap(), -23.0);
        close(r128_to_lufs("256").unwrap(), -24.0);
        close(r128_to_lufs("-256").unwrap(), -22.0);
        close(r128_to_lufs("-573").unwrap(), -20.7617);
        close(r128_to_lufs(" 1280 ").unwrap(), -28.0);
        close(r128_to_lufs("10240").unwrap(), -63.0);
        for text in ["", "abc", "1.5", "10241", "-10241", "99999999999"] {
            assert_eq!(r128_to_lufs(text), None, "{text:?}");
        }
    }

    #[test]
    fn peaks_become_decibels_below_full_scale() {
        close(peak_to_dbtp("1.0").unwrap(), 0.0);
        close(peak_to_dbtp("0.988553").unwrap(), -0.1005);
        close(peak_to_dbtp("0.5").unwrap(), -6.0206);
        close(peak_to_dbtp("2.0").unwrap(), 6.0206);
        for text in ["", "0", "-0.5", "NaN", "inf", "9", "peak"] {
            assert_eq!(peak_to_dbtp(text), None, "{text:?}");
        }
    }

    #[test]
    fn itunnorm_words_convert_per_the_documented_layout() {
        // The sample on id3.org: word 0 is 0x1E86 = 7814, so 10*log10(7.814) = 8.929 dB of cut.
        let sample = " 00001E86 00001E86 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211 00000211";
        let (lufs, peak) = parse_itunnorm(sample).unwrap();
        close(lufs, -18.0 + 8.929);
        close(peak.unwrap(), 20.0 * (30_970.0f64 / 32_768.0).log10());

        // Unity (1000) is 0 dB; the larger channel decides.
        let (lufs, _) = parse_itunnorm(" 000003E8 000003E8 0 0 0 0 0 0 0 0").unwrap();
        close(lufs, -18.0);
        let (lufs, _) = parse_itunnorm(" 000003E8 00000A5E 0 0 0 0 0 0 0 0").unwrap();
        close(lufs, -18.0 + 4.24);
        // A quiet track gets boosted: 0x0148 = 328 -> +4.84 dB.
        let (lufs, _) = parse_itunnorm("00000148 00000100 0 0 0 0 0 0 0 0").unwrap();
        close(lufs, -18.0 - 4.84);
        // Lower-case hex and no leading space are fine; no peak means none.
        let (_, peak) = parse_itunnorm("000003e8 000003e8 0 0 0 0 0 0 0 0").unwrap();
        assert_eq!(peak, None);
    }

    #[test]
    fn malformed_itunnorm_is_refused() {
        for text in [
            "",
            " 00001E86 00001E86",
            " 00001E86 00001E86 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211",
            " 00001E86 00001E86 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211 00000211 00000211",
            " 0000XXXX 00001E86 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211 00000211",
            // No adjustment recorded.
            " 00000000 00000000 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211 00000211",
            // 0xFFFFFFFF would be a 66 dB cut.
            " FFFFFFFF FFFFFFFF 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211 00000211",
        ] {
            assert_eq!(parse_itunnorm(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_ipod_soundcheck_value_follows_the_libgpod_equation() {
        assert_eq!(itunes_soundcheck(0.0), 1000);
        assert_eq!(itunes_soundcheck(-6.0), 3981);
        assert_eq!(itunes_soundcheck(6.0), 251);
        assert_eq!(itunes_soundcheck(-12.0), 15_849);
        assert_eq!(itunes_soundcheck(3.0), 501);
        assert_eq!(itunes_soundcheck(-100.0), u32::MAX, "saturates");
        assert_eq!(
            itunes_soundcheck(200.0),
            1,
            "never 0, which would switch Sound Check off"
        );
        assert_eq!(itunes_soundcheck(f64::NAN), 1000);
        assert_eq!(itunes_soundcheck(f64::INFINITY), 1000);
    }

    #[test]
    fn the_soundcheck_encoding_inverts_the_itunnorm_reading() {
        for word in [250u64, 328, 1000, 2654, 7814, 20_000] {
            let text = format!(" {word:08X} {word:08X} 0 0 0 0 0 0 0 0");
            let (lufs, _) = parse_itunnorm(&text).unwrap();
            let gain = REPLAYGAIN_REFERENCE_LUFS - lufs;
            let back = f64::from(itunes_soundcheck(gain));
            assert!(
                (back - word as f64).abs() <= 0.5 + word as f64 * 1e-9,
                "{word} -> {back}"
            );
        }
    }

    #[test]
    fn albums_combine_by_stated_value_or_by_length_weighted_energy() {
        let row = |lufs: Option<f64>, ms: u64| AlbumRow {
            lufs,
            peak: lufs.map(|_| -1.0),
            album_lufs: None,
            album_peak: None,
            duration_ms: ms,
        };
        // Equal lengths at -20 and -14 LUFS: the energy mean is 10*log10((0.01 + 0.039811)/2) = -16.037.
        let album = combine_album(&[row(Some(-20.0), 1000), row(Some(-14.0), 1000)]).unwrap();
        close(album.lufs, -16.037);
        assert_eq!(album.true_peak_dbtp, Some(-1.0));
        // A track three times as long counts three times as much.
        let weighted = combine_album(&[row(Some(-20.0), 3000), row(Some(-14.0), 1000)]).unwrap();
        close(
            weighted.lufs,
            10.0 * ((3.0 * 0.01 + 0.039_81) / 4.0f64).log10(),
        );
        // Any track unmeasured: nothing sensible to say.
        assert_eq!(
            combine_album(&[row(Some(-20.0), 1000), row(None, 1000)]),
            None
        );
        assert_eq!(combine_album(&[]), None);

        let mut stated = row(None, 1000);
        stated.album_lufs = Some(-17.0);
        stated.album_peak = Some(-0.5);
        let from_tags = combine_album(&[stated, row(None, 1000)]).unwrap();
        assert_eq!(
            (from_tags.lufs, from_tags.true_peak_dbtp),
            (-17.0, Some(-0.5))
        );
    }
}
