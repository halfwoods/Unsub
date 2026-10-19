/// What to do with a `Range` header for a resource of known length (RFC 9110 section 14).
#[derive(Debug, PartialEq, Eq)]
pub enum ByteRange {
    /// No usable range: serve the whole resource.
    Full,
    /// Inclusive byte positions.
    Partial {
        start: u64,
        end: u64,
    },
    Unsatisfiable,
}

/// Anything this does not understand (other units, several ranges, malformed numbers) is ignored,
/// which RFC 9110 permits and which makes the client fall back to a plain 200.
pub fn parse(header: Option<&str>, len: u64) -> ByteRange {
    let Some(spec) = header.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        return ByteRange::Full;
    };
    if spec.contains(',') {
        return ByteRange::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return ByteRange::Full;
    };
    let (first, last) = (first.trim(), last.trim());

    if first.is_empty() {
        let Ok(suffix) = last.parse::<u64>() else {
            return ByteRange::Full;
        };
        return if suffix == 0 || len == 0 {
            ByteRange::Unsatisfiable
        } else {
            ByteRange::Partial {
                start: len.saturating_sub(suffix),
                end: len - 1,
            }
        };
    }

    let Ok(start) = first.parse::<u64>() else {
        return ByteRange::Full;
    };
    let end = if last.is_empty() {
        u64::MAX
    } else {
        match last.parse::<u64>() {
            Ok(end) if end >= start => end,
            _ => return ByteRange::Full,
        }
    };
    if start >= len {
        return ByteRange::Unsatisfiable;
    }
    ByteRange::Partial {
        start,
        end: end.min(len - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(start: u64, end: u64) -> ByteRange {
        ByteRange::Partial { start, end }
    }

    #[test]
    fn explicit_open_ended_and_suffix_ranges() {
        assert_eq!(parse(Some("bytes=0-9"), 100), partial(0, 9));
        assert_eq!(parse(Some("bytes=90-"), 100), partial(90, 99));
        assert_eq!(parse(Some("bytes=-10"), 100), partial(90, 99));
        assert_eq!(parse(Some("bytes=5-5"), 100), partial(5, 5));
    }

    #[test]
    fn clamps_to_the_resource() {
        assert_eq!(parse(Some("bytes=50-5000"), 100), partial(50, 99));
        assert_eq!(parse(Some("bytes=-5000"), 100), partial(0, 99));
    }

    #[test]
    fn unsatisfiable_ranges() {
        assert_eq!(parse(Some("bytes=100-"), 100), ByteRange::Unsatisfiable);
        assert_eq!(parse(Some("bytes=200-300"), 100), ByteRange::Unsatisfiable);
        assert_eq!(parse(Some("bytes=-0"), 100), ByteRange::Unsatisfiable);
        assert_eq!(parse(Some("bytes=0-"), 0), ByteRange::Unsatisfiable);
        assert_eq!(parse(Some("bytes=-5"), 0), ByteRange::Unsatisfiable);
    }

    #[test]
    fn unusable_headers_are_ignored() {
        for header in [
            None,
            Some(""),
            Some("items=0-1"),
            Some("bytes=0-1,5-6"),
            Some("bytes=a-b"),
            Some("bytes=9-3"),
            Some("bytes=5"),
            Some("bytes=-"),
        ] {
            assert_eq!(parse(header, 100), ByteRange::Full, "{header:?}");
        }
    }

    #[test]
    fn tolerates_whitespace() {
        assert_eq!(parse(Some(" bytes= 1 - 2 "), 100), partial(1, 2));
    }
}
