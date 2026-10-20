//! Deciding whether `stream` sends a song as stored or has it converted.

use crate::{Song, StreamFormat};

/// What the client and the server's configuration ask for.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Request<'a> {
    /// The client's `format` parameter.
    pub format: Option<&'a str>,
    /// The client's `maxBitRate`; 0 means no limit.
    pub max_bitrate: Option<u32>,
    /// The server's cap for clients that name none.
    pub default_max: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Delivery {
    /// The song as stored, with `Range` support.
    Original,
    Convert {
        format: StreamFormat,
        max_kbps: Option<u32>,
    },
}

/// What a client names in `format`.
enum Asked {
    /// `raw`: no conversion at all, whatever else is configured.
    Raw,
    Format(StreamFormat),
    /// Nothing, or something this server cannot make (wma, say).
    Nothing,
}

impl Asked {
    fn parse(format: Option<&str>) -> Self {
        match format.map(str::to_ascii_lowercase).as_deref() {
            Some("raw") => Self::Raw,
            Some("mp3") => Self::Format(StreamFormat::Mp3),
            Some("aac" | "m4a") => Self::Format(StreamFormat::Aac),
            Some("opus" | "ogg" | "oga") => Self::Format(StreamFormat::Opus),
            Some("flac") => Self::Format(StreamFormat::Flac),
            _ => Self::Nothing,
        }
    }
}

impl Request<'_> {
    /// The cap in force: the client's, else the server's.
    fn cap(&self) -> Option<u32> {
        let positive = |kbps: Option<u32>| kbps.filter(|&kbps| kbps > 0);
        positive(self.max_bitrate).or(positive(self.default_max))
    }

    /// Whether anything could ask for a conversion. If not, the song need not even be looked up.
    pub fn may_convert(&self) -> bool {
        match Asked::parse(self.format) {
            Asked::Raw => false,
            Asked::Format(_) => true,
            Asked::Nothing => self.cap().is_some(),
        }
    }
}

/// The format a cap alone converts to: the one every client plays.
const CAP_FORMAT: StreamFormat = StreamFormat::Mp3;

pub(super) fn choose(request: &Request<'_>, song: &Song) -> Delivery {
    let asked = match Asked::parse(request.format) {
        Asked::Raw => return Delivery::Original,
        Asked::Format(format) => Some(format),
        Asked::Nothing => None,
    };
    let cap = request.cap();
    let stored = Stored::of(song);
    match asked {
        None => match cap {
            Some(cap) if stored.exceeds(cap) => Delivery::Convert {
                format: CAP_FORMAT,
                max_kbps: Some(cap),
            },
            _ => Delivery::Original,
        },
        Some(format) => {
            let differs = stored.format != Some(format);
            // A lossless target has no bitrate to hold down, so the same format is already right.
            let too_big =
                format != StreamFormat::Flac && cap.is_some_and(|cap| stored.exceeds(cap));
            if differs || too_big {
                Delivery::Convert {
                    format,
                    max_kbps: cap,
                }
            } else {
                Delivery::Original
            }
        }
    }
}

/// What the song is as stored, as far as the library says.
struct Stored {
    /// The streamable format it already is, if it is one.
    format: Option<StreamFormat>,
    lossless: bool,
    kbps: Option<u32>,
}

impl Stored {
    fn of(song: &Song) -> Self {
        let suffix = song
            .suffix
            .as_deref()
            .unwrap_or_default()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let kbps = song.bit_rate;
        // AAC never goes past 512 kbps, so an m4a above that is Apple Lossless.
        let alac = suffix == "m4a" && kbps.is_some_and(|kbps| kbps > 512);
        let lossless = alac
            || matches!(
                suffix.as_str(),
                "flac" | "wav" | "wave" | "aif" | "aiff" | "ape" | "wv" | "dsf" | "dff"
            );
        let format = match suffix.as_str() {
            "mp3" => Some(StreamFormat::Mp3),
            "aac" => Some(StreamFormat::Aac),
            "m4a" if !alac => Some(StreamFormat::Aac),
            "opus" => Some(StreamFormat::Opus),
            "flac" => Some(StreamFormat::Flac),
            // Ogg may hold Vorbis, which is none of the targets.
            _ => None,
        };
        Self {
            format,
            lossless,
            kbps,
        }
    }

    /// Whether streaming it as stored would go over `cap` kbps. A lossless song always does; a lossy
    /// one of unknown bitrate is given the benefit of the doubt.
    fn exceeds(&self, cap: u32) -> bool {
        self.lossless || self.kbps.is_some_and(|kbps| kbps > cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(suffix: &str, kbps: Option<u32>) -> Song {
        Song {
            suffix: Some(suffix.to_owned()),
            bit_rate: kbps,
            ..Song::default()
        }
    }

    fn request<'a>(format: Option<&'a str>, max: Option<u32>, default: Option<u32>) -> Request<'a> {
        Request {
            format,
            max_bitrate: max,
            default_max: default,
        }
    }

    fn convert(format: StreamFormat, max_kbps: Option<u32>) -> Delivery {
        Delivery::Convert { format, max_kbps }
    }

    use StreamFormat::{Aac, Flac, Mp3, Opus};

    #[test]
    fn nothing_asked_and_no_cap_is_the_original_without_a_lookup() {
        assert!(!request(None, None, None).may_convert());
        assert!(!request(Some("wma"), None, None).may_convert());
        assert!(!request(Some("raw"), Some(64), Some(64)).may_convert());
        assert_eq!(
            choose(&request(None, None, None), &song("flac", Some(900))),
            Delivery::Original
        );
    }

    #[test]
    fn raw_beats_every_cap() {
        for format in ["raw", "RAW"] {
            assert_eq!(
                choose(
                    &request(Some(format), Some(64), Some(96)),
                    &song("flac", Some(900))
                ),
                Delivery::Original
            );
        }
    }

    #[test]
    fn a_named_format_the_song_is_not_in_is_converted() {
        let mp3 = song("mp3", Some(320));
        assert_eq!(
            choose(&request(Some("aac"), None, None), &mp3),
            convert(Aac, None)
        );
        assert_eq!(
            choose(&request(Some("opus"), None, None), &mp3),
            convert(Opus, None)
        );
        assert_eq!(
            choose(&request(Some("flac"), None, None), &mp3),
            convert(Flac, None)
        );
        assert_eq!(
            choose(&request(Some("mp3"), None, None), &song("flac", Some(900))),
            convert(Mp3, None)
        );
    }

    #[test]
    fn format_names_have_aliases_and_any_case() {
        let flac = song("flac", Some(900));
        assert_eq!(
            choose(&request(Some("m4a"), None, None), &flac),
            convert(Aac, None)
        );
        assert_eq!(
            choose(&request(Some("ogg"), None, None), &flac),
            convert(Opus, None)
        );
        assert_eq!(
            choose(&request(Some("OPUS"), None, None), &flac),
            convert(Opus, None)
        );
    }

    #[test]
    fn the_same_format_is_left_alone_unless_it_is_over_the_cap() {
        let mp3 = song("mp3", Some(320));
        assert_eq!(
            choose(&request(Some("mp3"), None, None), &mp3),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(Some("mp3"), Some(320), None), &mp3),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(Some("mp3"), Some(128), None), &mp3),
            convert(Mp3, Some(128))
        );
        assert_eq!(
            choose(
                &request(Some("flac"), Some(128), None),
                &song("flac", Some(900))
            ),
            Delivery::Original
        );
    }

    #[test]
    fn a_cap_alone_converts_only_what_is_over_it() {
        assert_eq!(
            choose(&request(None, Some(128), None), &song("mp3", Some(320))),
            convert(Mp3, Some(128))
        );
        assert_eq!(
            choose(&request(None, Some(256), None), &song("mp3", Some(128))),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(None, Some(128), None), &song("mp3", Some(128))),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(None, Some(128), None), &song("m4a", Some(256))),
            convert(Mp3, Some(128))
        );
    }

    #[test]
    fn lossless_songs_are_always_over_a_cap() {
        for suffix in ["flac", "wav", "aiff", "ape", "wv"] {
            assert_eq!(
                choose(&request(None, Some(320), None), &song(suffix, None)),
                convert(Mp3, Some(320)),
                "{suffix}"
            );
        }
        // An m4a that fast can only be Apple Lossless.
        assert_eq!(
            choose(&request(None, Some(320), None), &song("m4a", Some(900))),
            convert(Mp3, Some(320))
        );
        assert_eq!(
            choose(&request(Some("aac"), None, None), &song("m4a", Some(900))),
            convert(Aac, None)
        );
        assert_eq!(
            choose(&request(Some("aac"), None, None), &song("m4a", Some(256))),
            Delivery::Original
        );
    }

    #[test]
    fn a_lossy_song_of_unknown_bitrate_is_not_converted_for_a_cap() {
        assert_eq!(
            choose(&request(None, Some(128), None), &song("mp3", None)),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(None, Some(128), None), &Song::default()),
            Delivery::Original
        );
    }

    #[test]
    fn the_servers_cap_stands_in_for_a_client_that_names_none() {
        let big = song("mp3", Some(320));
        assert_eq!(
            choose(&request(None, None, Some(192)), &big),
            convert(Mp3, Some(192))
        );
        assert_eq!(
            choose(&request(None, None, Some(192)), &song("mp3", Some(128))),
            Delivery::Original
        );
        // The client's own cap wins, up or down.
        assert_eq!(
            choose(&request(None, Some(96), Some(192)), &big),
            convert(Mp3, Some(96))
        );
        assert_eq!(
            choose(&request(None, Some(320), Some(192)), &big),
            Delivery::Original
        );
        // It applies to a named format too, which would otherwise be unbounded.
        assert_eq!(
            choose(&request(Some("aac"), None, Some(192)), &big),
            convert(Aac, Some(192))
        );
        assert!(request(None, None, Some(192)).may_convert());
    }

    #[test]
    fn zero_means_no_limit_for_the_client_and_for_the_server() {
        let big = song("mp3", Some(320));
        assert_eq!(
            choose(&request(None, Some(0), None), &big),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(None, None, Some(0)), &big),
            Delivery::Original
        );
        // A client's 0 is what most send for "original quality": the server's cap still applies.
        assert_eq!(
            choose(&request(None, Some(0), Some(192)), &big),
            convert(Mp3, Some(192))
        );
    }

    #[test]
    fn a_format_the_server_cannot_make_is_ignored() {
        let big = song("flac", Some(900));
        assert_eq!(
            choose(&request(Some("wma"), None, None), &big),
            Delivery::Original
        );
        assert_eq!(
            choose(&request(Some("wma"), Some(128), None), &big),
            convert(Mp3, Some(128))
        );
    }

    #[test]
    fn ogg_is_not_assumed_to_be_opus() {
        assert_eq!(
            choose(&request(Some("opus"), None, None), &song("ogg", Some(160))),
            convert(Opus, None)
        );
        assert_eq!(
            choose(&request(Some("opus"), None, None), &song("opus", Some(128))),
            Delivery::Original
        );
    }

    #[test]
    fn suffixes_may_come_with_a_dot_or_in_capitals() {
        assert_eq!(
            choose(&request(Some("mp3"), None, None), &song(".MP3", Some(256))),
            Delivery::Original
        );
    }
}
