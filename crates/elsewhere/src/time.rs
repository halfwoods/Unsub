/// Unix seconds for the timestamps the exports use: "2021-03-01T20:44:12Z", Spotify's older
/// "2021-03-01 20:44" (UTC, no seconds), Google's "2023-04-01T12:00:00.123Z" and
/// "2026-02-12T10:15:52+00:00". A time with no zone is taken as UTC.
pub(crate) fn unix(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', ' ']).unwrap_or((text, ""));
    let mut parts = date.splitn(3, '-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let zone_at = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    let mut fields = clock.splitn(3, ':');
    let hour: i64 = fields
        .next()
        .filter(|h| !h.is_empty())
        .map_or(Some(0), |h| h.parse().ok())?;
    let minute: i64 = fields.next().map_or(Some(0), |m| m.parse().ok())?;
    // Fractions of a second are dropped, so the same play always lands on the same second.
    let second: i64 = fields
        .next()
        .map_or(Some(0), |s| s.split('.').next()?.parse().ok())?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let local = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Some(local - offset_seconds(zone)?)
}

/// "Z" and "" are UTC; "+05:30", "-0800" and "+01" are hours and minutes east of it.
fn offset_seconds(zone: &str) -> Option<i64> {
    let Some(sign) = zone.chars().next().filter(|c| matches!(c, '+' | '-')) else {
        return (zone.is_empty() || zone == "Z").then_some(0);
    };
    let digits: String = zone[1..].chars().filter(char::is_ascii_digit).collect();
    let (hours, minutes) = match digits.len() {
        2 => (digits.parse::<i64>().ok()?, 0),
        4 => (
            digits[..2].parse::<i64>().ok()?,
            digits[2..].parse::<i64>().ok()?,
        ),
        _ => return None,
    };
    let seconds = hours * 3_600 + minutes * 60;
    Some(if sign == '-' { -seconds } else { seconds })
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_shape_the_exports_use() {
        assert_eq!(unix("2021-03-01T20:44:12Z"), Some(1_614_631_452));
        assert_eq!(unix("2021-03-01 20:44"), Some(1_614_631_440));
        assert_eq!(unix("2023-04-01T12:00:00.123Z"), Some(1_680_350_400));
        assert_eq!(unix("2026-02-12T10:15:52+00:00"), Some(1_770_891_352));
        assert_eq!(unix("2021-03-01T20:44:12"), Some(1_614_631_452));
        assert_eq!(unix("1970-01-01"), Some(0));
    }

    #[test]
    fn a_zone_moves_the_time_to_utc() {
        assert_eq!(unix("2021-03-01T20:44:12-05:00"), Some(1_614_649_452));
        assert_eq!(unix("2021-03-02T02:14:12+0530"), Some(1_614_631_452));
        assert_eq!(unix("2021-03-01T21:44:12+01"), Some(1_614_631_452));
    }

    #[test]
    fn leap_days_count() {
        assert_eq!(unix("2000-02-29T12:00:00Z"), Some(951_825_600));
    }

    #[test]
    fn nonsense_is_not_a_time() {
        for text in [
            "",
            "yesterday",
            "2021-13-01T00:00:00Z",
            "2021-03-01T25:00:00Z",
            "2021-03-01T20:44:12+9",
            "2021-03",
        ] {
            assert_eq!(unix(text), None, "{text:?}");
        }
    }
}
