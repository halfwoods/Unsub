use std::net::IpAddr;

use serde::Serialize;

use crate::media::AudioFormat;

/// Which protocol a speaker is driven with, which is also how the menu groups it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Chromecast,
    /// A UPnP renderer that says it is made by Sonos. Driven like any other, but it is known to
    /// take what a bare renderer might not, such as queueing the next song.
    Sonos,
    /// Any other UPnP / DLNA renderer.
    Upnp,
}

impl Kind {
    /// Whether this kind of speaker plays `format` as it is. What it does not play is converted
    /// to FLAC at [`Kind::lossless_limits`] first.
    ///
    /// The limits are the cautious ones: 48 kHz is what every Chromecast and Sonos takes, and
    /// community reports have 24-bit FLAC above it failing on Sonos and on generic renderers. WAV is
    /// kept to 16 bits, which is all renderers agree on. Whether the very newest speakers would take
    /// more is not worth a song that doesn't play.
    pub fn plays(self, format: AudioFormat) -> bool {
        use crate::media::Codec::{Aac, Aiff, Alac, Flac, Mp3, Opus, Other, Vorbis, Wav};
        let (rate, depth) = (format.sample_rate, format.bit_depth);
        let within = |bits: u8| {
            rate.is_none_or(|rate| rate <= MAX_RATE) && depth.is_none_or(|depth| depth <= bits)
        };
        match format.codec {
            Mp3 | Aac | Vorbis => within(u8::MAX),
            Flac => within(MAX_DEPTH),
            Wav => within(16),
            Opus => self == Self::Chromecast,
            Alac | Aiff | Other => false,
        }
    }
}

/// Speakers take nothing above this rate or depth without a conversion.
pub const MAX_RATE: u32 = 48_000;
pub const MAX_DEPTH: u8 = 24;

/// How to reach a speaker, once it has been found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Chromecast { port: u16 },
    Upnp(UpnpEndpoint),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpnpEndpoint {
    pub av_transport: Service,
    pub rendering_control: Option<Service>,
    /// Sonos only: says which speakers are bonded or grouped together, and who commands them.
    pub zone_topology: Option<Service>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Service {
    /// The full service type, e.g. `urn:schemas-upnp-org:service:AVTransport:1`, which SOAP
    /// requests name.
    pub service_type: String,
    /// Absolute.
    pub control_url: String,
}

/// A speaker that can be played to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable between searches and sessions, so a menu can keep its checkmark: `cast:<uuid>` or
    /// `upnp:<UDN>`.
    pub id: String,
    /// What its owner called it, such as "Kitchen".
    pub name: String,
    pub kind: Kind,
    #[serde(skip)]
    pub address: IpAddr,
    #[serde(skip)]
    pub(crate) endpoint: Endpoint,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::Codec;

    fn format(codec: Codec, rate: u32, depth: u8) -> AudioFormat {
        AudioFormat {
            codec,
            sample_rate: Some(rate),
            bit_depth: Some(depth),
        }
    }

    #[test]
    fn everyday_formats_play_as_they_are() {
        for kind in [Kind::Chromecast, Kind::Sonos, Kind::Upnp] {
            assert!(kind.plays(format(Codec::Mp3, 44_100, 16)), "{kind:?}");
            assert!(kind.plays(format(Codec::Aac, 44_100, 16)), "{kind:?}");
            assert!(kind.plays(format(Codec::Flac, 44_100, 16)), "{kind:?}");
            assert!(kind.plays(format(Codec::Flac, 48_000, 24)), "{kind:?}");
            assert!(kind.plays(format(Codec::Vorbis, 44_100, 16)), "{kind:?}");
        }
    }

    #[test]
    fn high_resolution_and_unsupported_codecs_need_converting() {
        for kind in [Kind::Chromecast, Kind::Sonos, Kind::Upnp] {
            assert!(!kind.plays(format(Codec::Flac, 96_000, 24)), "{kind:?}");
            assert!(!kind.plays(format(Codec::Flac, 44_100, 32)), "{kind:?}");
            assert!(!kind.plays(format(Codec::Alac, 44_100, 16)), "{kind:?}");
            assert!(!kind.plays(format(Codec::Aiff, 44_100, 16)), "{kind:?}");
            assert!(!kind.plays(format(Codec::Other, 44_100, 16)), "{kind:?}");
            assert!(!kind.plays(format(Codec::Wav, 44_100, 24)), "{kind:?}");
        }
    }

    #[test]
    fn only_chromecasts_play_opus() {
        let opus = format(Codec::Opus, 48_000, 16);
        assert!(Kind::Chromecast.plays(opus));
        assert!(!Kind::Sonos.plays(opus));
        assert!(!Kind::Upnp.plays(opus));
    }

    #[test]
    fn a_format_with_unknown_rate_or_depth_is_given_the_benefit_of_the_doubt() {
        let lossy = AudioFormat {
            codec: Codec::Mp3,
            sample_rate: None,
            bit_depth: None,
        };
        assert!(Kind::Sonos.plays(lossy));
    }

    #[test]
    fn devices_serialize_without_their_network_details() {
        let device = Device {
            id: "cast:abc".into(),
            name: "Kitchen".into(),
            kind: Kind::Chromecast,
            address: "192.168.1.5".parse().unwrap(),
            endpoint: Endpoint::Chromecast { port: 8009 },
        };
        assert_eq!(
            serde_json::to_value(&device).unwrap(),
            serde_json::json!({ "id": "cast:abc", "name": "Kitchen", "kind": "chromecast" })
        );
    }
}
