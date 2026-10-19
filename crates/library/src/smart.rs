use rusqlite::Connection;
use rusqlite::types::ToSql;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::normalize::fold;
use crate::playlist::{members_sql, playlist_row};

const DAY_SECONDS: i64 = 86_400;

/// Mirrors iTunes 10's smart playlist editor. Only the top level's `limit`,
/// `only_checked` and `live` apply; a nested group is just its rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmartRules {
    pub match_all: bool,
    pub rules: Vec<SmartRule>,
    pub limit: Option<SmartLimit>,
    pub only_checked: bool,
    /// When false the playlist is a snapshot taken whenever its rules are set.
    pub live: bool,
}

impl Default for SmartRules {
    fn default() -> Self {
        Self {
            match_all: true,
            rules: Vec::new(),
            limit: None,
            only_checked: false,
            live: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SmartRule {
    Condition {
        field: SmartField,
        op: SmartOp,
        value: SmartValue,
    },
    /// Option-click in iTunes: a parenthesised sub-expression.
    Group(SmartRules),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SmartField {
    Name,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Composer,
    /// The codec, e.g. "ALAC".
    Kind,
    Year,
    Rating,
    Plays,
    Skips,
    LastPlayed,
    LastSkipped,
    DateAdded,
    Time,
    BitRate,
    SampleRate,
    Size,
    /// The song checkbox.
    Checked,
    TrackNumber,
    DiscNumber,
    Compilation,
    /// Membership of another playlist.
    Playlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SmartOp {
    Contains,
    DoesNotContain,
    Is,
    IsNot,
    StartsWith,
    EndsWith,
    GreaterThan,
    LessThan,
    InRange,
    Before,
    After,
    InTheLast,
    NotInTheLast,
    IsTrue,
    IsFalse,
}

/// Numbers use storage units: rating 0..=100, time in milliseconds, size in
/// bytes, dates as unix seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SmartValue {
    Text(String),
    Number(i64),
    /// Inclusive on both ends.
    Range {
        from: i64,
        to: i64,
    },
    Relative {
        amount: u32,
        unit: DateUnit,
    },
    Playlist(i64),
    /// For true/false conditions, which take no operand.
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DateUnit {
    Days,
    Weeks,
    Months,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartLimit {
    pub amount: u32,
    pub unit: LimitUnit,
    pub selected_by: LimitOrder,
}

/// Megabytes and gigabytes are decimal (10^6, 10^9 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LimitUnit {
    Items,
    Minutes,
    Hours,
    Megabytes,
    Gigabytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LimitOrder {
    Random,
    Album,
    Artist,
    Genre,
    Name,
    HighestRating,
    LowestRating,
    MostRecentlyPlayed,
    LeastRecentlyPlayed,
    MostOftenPlayed,
    LeastOftenPlayed,
    MostRecentlyAdded,
    LeastRecentlyAdded,
}

enum FieldKind {
    Text(&'static str),
    Number(&'static str),
    Date(&'static str),
    Flag(&'static str),
    Playlist,
}

impl SmartField {
    fn kind(self) -> FieldKind {
        match self {
            Self::Name => FieldKind::Text("title"),
            Self::Artist => FieldKind::Text("artist"),
            Self::AlbumArtist => FieldKind::Text("album_artist"),
            Self::Album => FieldKind::Text("album"),
            Self::Genre => FieldKind::Text("genre"),
            Self::Composer => FieldKind::Text("composer"),
            Self::Kind => FieldKind::Text("codec"),
            Self::Year => FieldKind::Number("year"),
            Self::Rating => FieldKind::Number("rating"),
            Self::Plays => FieldKind::Number("play_count"),
            Self::Skips => FieldKind::Number("skip_count"),
            Self::Time => FieldKind::Number("duration_ms"),
            Self::BitRate => FieldKind::Number("bitrate_kbps"),
            Self::SampleRate => FieldKind::Number("sample_rate"),
            Self::Size => FieldKind::Number("file_size"),
            Self::TrackNumber => FieldKind::Number("track_number"),
            Self::DiscNumber => FieldKind::Number("disc_number"),
            Self::LastPlayed => FieldKind::Date("last_played"),
            Self::LastSkipped => FieldKind::Date("last_skipped"),
            Self::DateAdded => FieldKind::Date("date_added"),
            Self::Checked => FieldKind::Flag("enabled"),
            Self::Compilation => FieldKind::Flag("compilation"),
            Self::Playlist => FieldKind::Playlist,
        }
    }
}

/// Collects bound parameters in the order their placeholders are written, so
/// fragments can be spliced together without tracking indexes.
pub(crate) struct Compile {
    pub params: Vec<Box<dyn ToSql>>,
    now: i64,
    /// Playlists being expanded; meeting one again means the rules loop.
    pub stack: Vec<i64>,
    /// Stored rules may name playlists deleted since, and that must not break
    /// listing; rules being saved may not.
    lenient: bool,
}

impl Compile {
    pub(crate) fn new(now: i64, lenient: bool) -> Self {
        Self {
            params: Vec::new(),
            now,
            stack: Vec::new(),
            lenient,
        }
    }

    pub(crate) fn bind(&mut self, value: impl ToSql + 'static) {
        self.params.push(Box::new(value));
    }
}

/// A `SELECT id ...` over the tracks the rules pick, honouring the limit.
/// `ordered` also orders the ids as the limit selected them, for snapshots.
pub(crate) fn smart_ids_sql(
    conn: &Connection,
    rules: &SmartRules,
    seed: i64,
    c: &mut Compile,
    ordered: bool,
) -> Result<String> {
    let mut filter = rules_sql(conn, rules, c)?;
    if rules.only_checked {
        filter = format!("({filter}) AND enabled = 1");
    }
    let Some(limit) = rules.limit else {
        let order = if ordered { " ORDER BY id" } else { "" };
        return Ok(format!("SELECT id FROM tracks WHERE {filter}{order}"));
    };

    let order = selection_order(limit.selected_by, seed);
    let (measure, per_unit) = match limit.unit {
        // A top-N sort is far cheaper than a running total over every match.
        LimitUnit::Items => {
            c.bind(i64::from(limit.amount));
            return Ok(format!(
                "SELECT id FROM tracks WHERE {filter} ORDER BY {order} LIMIT ?"
            ));
        }
        LimitUnit::Minutes => ("duration_ms", 60_000),
        LimitUnit::Hours => ("duration_ms", 3_600_000),
        LimitUnit::Megabytes => ("file_size", 1_000_000),
        LimitUnit::Gigabytes => ("file_size", 1_000_000_000),
    };
    c.bind(i64::from(limit.amount) * per_unit);
    let ordered = if ordered { " ORDER BY running" } else { "" };
    // Tracks join in the chosen order until the next one would overshoot; the
    // running total only grows, so what passes is always a prefix.
    Ok(format!(
        "SELECT id FROM (
            SELECT id, SUM({measure}) OVER (
                ORDER BY {order} ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
            ) AS running FROM tracks WHERE {filter}
        ) WHERE running <= ?{ordered}"
    ))
}

/// A multiplicative hash of the id and the playlist's seed: stable until the
/// seed is re-rolled, and unlike `random()` it can be re-evaluated per query.
fn selection_order(selected_by: LimitOrder, seed: i64) -> String {
    const BY_ARTIST: &str = "key_artist ASC NULLS LAST, key_album ASC NULLS LAST, \
                             disc_number ASC NULLS LAST, track_number ASC NULLS LAST";
    let primary = match selected_by {
        LimitOrder::Random => format!("(id * 2654435761 + {seed} * 40503) % 4294967296 ASC"),
        LimitOrder::Album => {
            "key_album ASC NULLS LAST, disc_number ASC NULLS LAST, track_number ASC NULLS LAST"
                .to_owned()
        }
        LimitOrder::Artist => BY_ARTIST.to_owned(),
        LimitOrder::Genre => format!("key_genre ASC NULLS LAST, {BY_ARTIST}"),
        LimitOrder::Name => "key_title ASC NULLS LAST".to_owned(),
        LimitOrder::HighestRating => "rating DESC".to_owned(),
        LimitOrder::LowestRating => "rating ASC".to_owned(),
        LimitOrder::MostRecentlyPlayed => "last_played DESC NULLS LAST".to_owned(),
        LimitOrder::LeastRecentlyPlayed => "last_played ASC NULLS FIRST".to_owned(),
        LimitOrder::MostOftenPlayed => "play_count DESC".to_owned(),
        LimitOrder::LeastOftenPlayed => "play_count ASC".to_owned(),
        LimitOrder::MostRecentlyAdded => "date_added DESC".to_owned(),
        LimitOrder::LeastRecentlyAdded => "date_added ASC".to_owned(),
    };
    format!("{primary}, id ASC")
}

/// An empty rule list matches everything, whichever way it combines.
fn rules_sql(conn: &Connection, rules: &SmartRules, c: &mut Compile) -> Result<String> {
    if rules.rules.is_empty() {
        return Ok("1".to_owned());
    }
    let parts = rules
        .rules
        .iter()
        .map(|rule| {
            let sql = match rule {
                SmartRule::Condition { field, op, value } => {
                    condition(conn, *field, *op, value, c)?
                }
                SmartRule::Group(group) => rules_sql(conn, group, c)?,
            };
            Ok(format!("({sql})"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join(if rules.match_all { " AND " } else { " OR " }))
}

fn condition(
    conn: &Connection,
    field: SmartField,
    op: SmartOp,
    value: &SmartValue,
    c: &mut Compile,
) -> Result<String> {
    let mismatch = || {
        Error::InvalidRule(format!(
            "{field:?} cannot be tested with {op:?} and {value:?}"
        ))
    };
    match field.kind() {
        FieldKind::Text(column) => text_condition(column, op, value, c).ok_or_else(mismatch),
        FieldKind::Number(column) => number_condition(column, op, value, c).ok_or_else(mismatch),
        FieldKind::Date(column) => date_condition(column, op, value, c).ok_or_else(mismatch),
        FieldKind::Flag(column) => match op {
            SmartOp::IsTrue => Ok(format!("{column} = 1")),
            SmartOp::IsFalse => Ok(format!("{column} = 0")),
            _ => Err(mismatch()),
        },
        FieldKind::Playlist => {
            let (SmartOp::Is | SmartOp::IsNot, SmartValue::Playlist(id)) = (op, value) else {
                return Err(mismatch());
            };
            let members = match playlist_row(conn, *id)? {
                Some(row) => members_sql(conn, &row, c)?,
                None if c.lenient => "SELECT id FROM tracks WHERE 0".to_owned(),
                None => return Err(Error::PlaylistNotFound(*id)),
            };
            let negation = if op == SmartOp::IsNot { "NOT " } else { "" };
            Ok(format!("id {negation}IN ({members})"))
        }
    }
}

/// `None` means the operator or operand does not suit the field.
fn text_condition(
    column: &str,
    op: SmartOp,
    value: &SmartValue,
    c: &mut Compile,
) -> Option<String> {
    let SmartValue::Text(text) = value else {
        return None;
    };
    let needle = fold(text);
    let column = format!("fold({column})");
    let length = i64::try_from(needle.chars().count()).ok()?;
    let sql = match op {
        SmartOp::Contains => format!("instr({column}, ?) > 0"),
        SmartOp::DoesNotContain => format!("instr({column}, ?) = 0"),
        SmartOp::Is => format!("{column} = ?"),
        SmartOp::IsNot => format!("{column} <> ?"),
        SmartOp::StartsWith | SmartOp::EndsWith if needle.is_empty() => {
            return Some("1".to_owned());
        }
        SmartOp::StartsWith => {
            c.bind(length);
            format!("substr({column}, 1, ?) = ?")
        }
        SmartOp::EndsWith => {
            c.bind(-length);
            format!("substr({column}, ?) = ?")
        }
        _ => return None,
    };
    c.bind(needle);
    Some(sql)
}

/// `IS` rather than `=` so a missing value counts as "is not".
fn number_condition(
    column: &str,
    op: SmartOp,
    value: &SmartValue,
    c: &mut Compile,
) -> Option<String> {
    Some(match (op, value) {
        (SmartOp::Is, SmartValue::Number(n)) => {
            c.bind(*n);
            format!("{column} IS ?")
        }
        (SmartOp::IsNot, SmartValue::Number(n)) => {
            c.bind(*n);
            format!("{column} IS NOT ?")
        }
        (SmartOp::GreaterThan, SmartValue::Number(n)) => {
            c.bind(*n);
            format!("{column} > ?")
        }
        (SmartOp::LessThan, SmartValue::Number(n)) => {
            c.bind(*n);
            format!("{column} < ?")
        }
        (SmartOp::InRange, SmartValue::Range { from, to }) => {
            c.bind(*from.min(to));
            c.bind(*from.max(to));
            format!("{column} BETWEEN ? AND ?")
        }
        _ => return None,
    })
}

/// "Is", "before" and "after" work on whole UTC days, as iTunes' date
/// pickers do; callers wanting local days use a range.
fn date_condition(
    column: &str,
    op: SmartOp,
    value: &SmartValue,
    c: &mut Compile,
) -> Option<String> {
    Some(match (op, value) {
        (SmartOp::Is, SmartValue::Number(t)) => {
            let start = start_of_day(*t);
            c.bind(start);
            c.bind(start + DAY_SECONDS);
            format!("({column} >= ? AND {column} < ?)")
        }
        (SmartOp::Before, SmartValue::Number(t)) => {
            c.bind(start_of_day(*t));
            format!("{column} < ?")
        }
        (SmartOp::After, SmartValue::Number(t)) => {
            c.bind(start_of_day(*t) + DAY_SECONDS);
            format!("{column} >= ?")
        }
        (SmartOp::InRange, SmartValue::Range { from, to }) => {
            c.bind(*from.min(to));
            c.bind(*from.max(to));
            format!("{column} BETWEEN ? AND ?")
        }
        (SmartOp::InTheLast, SmartValue::Relative { amount, unit }) => {
            c.bind(seconds_ago(c.now, *amount, *unit));
            format!("{column} >= ?")
        }
        // A track never played is not in the last two weeks either.
        (SmartOp::NotInTheLast, SmartValue::Relative { amount, unit }) => {
            c.bind(seconds_ago(c.now, *amount, *unit));
            format!("({column} IS NULL OR {column} < ?)")
        }
        _ => return None,
    })
}

fn start_of_day(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(DAY_SECONDS)
}

fn seconds_ago(now: i64, amount: u32, unit: DateUnit) -> i64 {
    match unit {
        DateUnit::Days => now - i64::from(amount) * DAY_SECONDS,
        DateUnit::Weeks => now - i64::from(amount) * 7 * DAY_SECONDS,
        DateUnit::Months => months_before(now, amount),
    }
}

/// Calendar months, clamping to the month's last day (31 March minus a month
/// is 28 or 29 February), in UTC.
fn months_before(timestamp: i64, months: u32) -> i64 {
    let (year, month, day) = civil_from_days(timestamp.div_euclid(DAY_SECONDS));
    let total = year * 12 + (month - 1) - i64::from(months);
    let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    let day = day.min(days_in_month(year, month));
    days_from_civil(year, month, day) * DAY_SECONDS + timestamp.rem_euclid(DAY_SECONDS)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's civil-date algorithms (public domain), proleptic Gregorian.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
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

    fn at(year: i64, month: i64, day: i64, hour: i64) -> i64 {
        days_from_civil(year, month, day) * DAY_SECONDS + hour * 3_600
    }

    #[test]
    fn civil_conversions_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(
            civil_from_days(days_from_civil(1969, 12, 31)),
            (1969, 12, 31)
        );
    }

    #[test]
    fn months_before_clamps_to_the_shorter_month() {
        let cases = [
            ((2024, 3, 31, 10), 1, (2024, 2, 29, 10)),
            ((2023, 3, 31, 10), 1, (2023, 2, 28, 10)),
            ((2024, 1, 15, 0), 2, (2023, 11, 15, 0)),
            ((2024, 5, 31, 23), 1, (2024, 4, 30, 23)),
            ((2024, 6, 10, 5), 12, (2023, 6, 10, 5)),
            ((2024, 6, 10, 5), 0, (2024, 6, 10, 5)),
        ];
        for ((y, m, d, h), months, (ey, em, ed, eh)) in cases {
            assert_eq!(
                months_before(at(y, m, d, h), months),
                at(ey, em, ed, eh),
                "{y}-{m}-{d} minus {months} months"
            );
        }
    }

    #[test]
    fn rules_round_trip_through_json_in_the_shape_the_ui_edits() {
        let rules = SmartRules {
            match_all: false,
            rules: vec![
                SmartRule::Condition {
                    field: SmartField::Genre,
                    op: SmartOp::Is,
                    value: SmartValue::Text("Classical".into()),
                },
                SmartRule::Group(SmartRules {
                    rules: vec![SmartRule::Condition {
                        field: SmartField::Year,
                        op: SmartOp::InRange,
                        value: SmartValue::Range {
                            from: 1990,
                            to: 1999,
                        },
                    }],
                    ..SmartRules::default()
                }),
            ],
            limit: Some(SmartLimit {
                amount: 25,
                unit: LimitUnit::Items,
                selected_by: LimitOrder::MostOftenPlayed,
            }),
            only_checked: true,
            live: false,
        };

        let json = serde_json::to_value(&rules).unwrap();

        assert_eq!(json["rules"][0]["type"], "condition");
        assert_eq!(json["rules"][0]["value"]["text"], "Classical");
        assert_eq!(json["rules"][1]["type"], "group");
        assert_eq!(json["rules"][1]["matchAll"], true);
        assert_eq!(json["limit"]["selectedBy"], "mostOftenPlayed");
        assert_eq!(serde_json::from_value::<SmartRules>(json).unwrap(), rules);
    }

    #[test]
    fn omitted_rule_settings_take_the_defaults() {
        let rules: SmartRules = serde_json::from_str(r#"{"rules":[]}"#).unwrap();

        assert_eq!(rules, SmartRules::default());
        assert!(rules.match_all && rules.live && !rules.only_checked);
    }
}
