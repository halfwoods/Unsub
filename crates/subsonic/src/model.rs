use serde::{Serialize, Serializer};

/// Fields mirror the Subsonic `ArtistID3` type. Optional fields are omitted from responses when `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub album_count: u32,
    pub cover_art: Option<String>,
}

/// Mirrors `AlbumID3`. `duration` is seconds and `created` is seconds since the Unix epoch.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub song_count: u32,
    pub duration: u32,
    pub cover_art: Option<String>,
    #[serde(serialize_with = "iso8601")]
    pub created: i64,
}

/// Mirrors the Subsonic `Child` type for a song (the server adds `isDir`, `isVideo` and `type`).
/// `size` is bytes, `duration` seconds, `bit_rate` kbps and `created` seconds since the Unix epoch.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub parent: Option<String>,
    pub title: String,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub suffix: Option<String>,
    pub duration: Option<u32>,
    pub bit_rate: Option<u32>,
    pub path: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub cover_art: Option<String>,
    pub play_count: Option<u64>,
    #[serde(serialize_with = "iso8601_opt")]
    pub created: Option<i64>,
}

/// `duration` is seconds; `created` and `changed` are seconds since the Unix epoch.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub song_count: u32,
    pub duration: u32,
    #[serde(serialize_with = "iso8601")]
    pub created: i64,
    #[serde(serialize_with = "iso8601")]
    pub changed: i64,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub value: String,
    pub song_count: u32,
    pub album_count: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchResult {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub songs: Vec<Song>,
}

fn iso8601<S: Serializer>(secs: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_iso8601(*secs))
}

fn iso8601_opt<S: Serializer>(secs: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error> {
    match secs {
        Some(secs) => iso8601(secs, serializer),
        None => serializer.serialize_none(),
    }
}

/// Subsonic timestamps are ISO 8601 in UTC, e.g. `2023-11-14T22:13:20Z`.
fn format_iso8601(secs: i64) -> String {
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let in_day = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        in_day / 3_600,
        in_day % 3_600 / 60,
        in_day % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_iso8601(-1), "1969-12-31T23:59:59Z");
    }
}
