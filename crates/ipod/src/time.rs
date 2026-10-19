use chrono::{DateTime, Local, TimeZone};

/// Seconds between the Mac epoch (1904-01-01) and the Unix epoch.
const MAC_EPOCH_OFFSET: i64 = 2_082_844_800;

/// iTunesDB stores seconds since 1904 on the wall clock of the machine that wrote them, not in UTC.
/// libgpod (`device_time_mac_to_time_t`) subtracts the local UTC offset; resolving the wall-clock
/// time through the zone rules instead keeps timestamps from the other side of a DST change right.
pub(crate) fn mac_to_unix(mac: u32) -> Option<i64> {
    mac_to_unix_in(mac, &Local)
}

fn mac_to_unix_in<Tz: TimeZone>(mac: u32, zone: &Tz) -> Option<i64> {
    if mac == 0 {
        return None;
    }
    let wall_clock = DateTime::from_timestamp(i64::from(mac) - MAC_EPOCH_OFFSET, 0)?.naive_utc();
    // A wall-clock time skipped by a DST jump has no local instant; read it as UTC instead.
    let unix = zone
        .from_local_datetime(&wall_clock)
        .earliest()
        .map_or_else(
            || wall_clock.and_utc().timestamp(),
            |local| local.timestamp(),
        );
    Some(unix)
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, Utc};

    use super::*;

    const UNIX_EPOCH_AS_MAC: u32 = 2_082_844_800;

    #[test]
    fn zero_means_never() {
        assert_eq!(mac_to_unix_in(0, &Utc), None);
    }

    #[test]
    fn utc_wall_clock_maps_directly() {
        assert_eq!(
            mac_to_unix_in(UNIX_EPOCH_AS_MAC + 86_400, &Utc),
            Some(86_400)
        );
    }

    #[test]
    fn wall_clock_east_of_utc_is_earlier_in_unix_time() {
        let plus_two = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(
            mac_to_unix_in(UNIX_EPOCH_AS_MAC + 86_400, &plus_two),
            Some(86_400 - 7200)
        );
    }

    #[test]
    fn dates_before_1970_are_negative() {
        assert_eq!(mac_to_unix_in(UNIX_EPOCH_AS_MAC - 60, &Utc), Some(-60));
    }
}
