//! Identifying a disc through the MusicBrainz web service.

use serde::Deserialize;

use crate::{Disc, Error, MetadataClient, Result};

/// One release (an album as issued) that MusicBrainz knows this disc belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub mbid: String,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    /// Which disc of the release this is (1-based) and how many it has.
    pub disc_number: u32,
    pub disc_count: u32,
    /// The inserted disc's tracks, in order.
    pub tracks: Vec<ReleaseTrack>,
    /// A front cover on the Cover Art Archive, when the release has one.
    pub cover_art_url: Option<String>,
    pub release_group_mbid: Option<String>,
    pub artist_mbid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTrack {
    /// Position on the disc, from 1.
    pub position: u32,
    pub title: String,
    pub artist: String,
    pub recording_mbid: Option<String>,
    pub track_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub length_ms: Option<u32>,
}

/// Looks the disc up by its DiscID. Several releases can share one disc (reissues, regional
/// editions); they come back in MusicBrainz's order for the caller to choose from. A disc
/// MusicBrainz has never seen is an empty list, not an error.
pub fn lookup(disc: &Disc, client: &dyn MetadataClient) -> Result<Vec<Release>> {
    let url = format!(
        "https://musicbrainz.org/ws/2/discid/{}?inc=recordings+artist-credits+release-groups&fmt=json",
        disc.id
    );
    match client.get(&url)? {
        Some(body) => parse(&body, &disc.id),
        None => Ok(Vec::new()),
    }
}

fn parse(body: &[u8], disc_id: &str) -> Result<Vec<Release>> {
    let response: Response = serde_json::from_slice(body).map_err(|err| Error::Response {
        service: "MusicBrainz",
        detail: err.to_string(),
    })?;
    Ok(response
        .releases
        .into_iter()
        .filter_map(|release| release.into_release(disc_id))
        .collect())
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    releases: Vec<RawRelease>,
}

#[derive(Deserialize)]
struct RawRelease {
    id: String,
    #[serde(default)]
    title: String,
    date: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<Credit>,
    #[serde(default)]
    media: Vec<Medium>,
    #[serde(rename = "cover-art-archive")]
    cover_art: Option<CoverArt>,
    #[serde(rename = "release-group")]
    release_group: Option<Id>,
}

#[derive(Deserialize)]
struct Medium {
    position: Option<u32>,
    #[serde(default)]
    discs: Vec<Id>,
    #[serde(default)]
    tracks: Vec<RawTrack>,
}

#[derive(Deserialize)]
struct RawTrack {
    id: Option<String>,
    position: Option<u32>,
    title: Option<String>,
    length: Option<u32>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<Credit>,
    recording: Option<Recording>,
}

#[derive(Deserialize)]
struct Recording {
    id: Option<String>,
    title: Option<String>,
    length: Option<u32>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<Credit>,
}

#[derive(Deserialize)]
struct Credit {
    #[serde(default)]
    name: String,
    #[serde(default)]
    joinphrase: String,
    artist: Option<Id>,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct CoverArt {
    #[serde(default)]
    front: bool,
}

impl RawRelease {
    /// `None` when none of the release's media is the disc being looked up.
    fn into_release(self, disc_id: &str) -> Option<Release> {
        let disc_count = u32::try_from(self.media.len()).unwrap_or(u32::MAX);
        let (index, medium) = self
            .media
            .into_iter()
            .enumerate()
            .find(|(_, medium)| medium.discs.iter().any(|d| d.id == disc_id))?;
        let artist = credited_name(&self.artist_credit);
        let tracks = medium
            .tracks
            .into_iter()
            .enumerate()
            .map(|(index, track)| track.into_track(index, &self.artist_credit))
            .collect();
        Some(Release {
            // The id goes into a URL, so only an id shaped like a UUID is trusted to.
            cover_art_url: (self.cover_art.is_some_and(|art| art.front) && is_uuid_like(&self.id))
                .then(|| format!("https://coverartarchive.org/release/{}/front-500", self.id)),
            mbid: self.id,
            title: self.title,
            artist,
            year: self.date.as_deref().and_then(year_of),
            disc_number: medium
                .position
                .unwrap_or(u32::try_from(index + 1).unwrap_or(1)),
            disc_count,
            tracks,
            release_group_mbid: self.release_group.map(|group| group.id),
            artist_mbid: first_artist_id(&self.artist_credit),
        })
    }
}

impl RawTrack {
    fn into_track(self, index: usize, release_credit: &[Credit]) -> ReleaseTrack {
        let recording = self.recording.unwrap_or(Recording {
            id: None,
            title: None,
            length: None,
            artist_credit: Vec::new(),
        });
        // A track's own credit wins over the recording's, which wins over the album's.
        let credit = [
            self.artist_credit.as_slice(),
            recording.artist_credit.as_slice(),
            release_credit,
        ]
        .into_iter()
        .find(|credit| !credit.is_empty())
        .unwrap_or_default();
        ReleaseTrack {
            position: self
                .position
                .unwrap_or(u32::try_from(index + 1).unwrap_or(1)),
            title: self.title.or(recording.title).unwrap_or_default(),
            artist: credited_name(credit),
            recording_mbid: recording.id,
            track_mbid: self.id,
            artist_mbid: first_artist_id(credit),
            length_ms: self.length.or(recording.length),
        }
    }
}

/// "Simon" + " & " + "Garfunkel": names with their join phrases, as MusicBrainz displays them.
fn credited_name(credit: &[Credit]) -> String {
    credit
        .iter()
        .map(|part| format!("{}{}", part.name, part.joinphrase))
        .collect()
}

fn first_artist_id(credit: &[Credit]) -> Option<String> {
    credit
        .first()?
        .artist
        .as_ref()
        .map(|artist| artist.id.clone())
}

fn is_uuid_like(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Dates are `YYYY`, `YYYY-MM` or `YYYY-MM-DD`.
fn year_of(date: &str) -> Option<i32> {
    date.get(..4)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Mutex};

    use super::*;
    use crate::Toc;

    const GUANO_APES: &[u8] =
        include_bytes!("../tests/fixtures/musicbrainz_discid_guano_apes.json");
    const TWO_DISCS: &[u8] = include_bytes!("../tests/fixtures/musicbrainz_discid_two_discs.json");

    fn disc(id: &str) -> Disc {
        Disc {
            id: id.to_owned(),
            name: "Audio CD".into(),
            toc: Toc {
                first_track: 1,
                last_track: 1,
                lead_out: 1000,
                offsets: vec![150],
            },
            tracks: Vec::new(),
            mount: PathBuf::new(),
        }
    }

    /// Answers one URL and remembers what was asked.
    struct Canned {
        body: Option<&'static [u8]>,
        asked: Mutex<Vec<String>>,
    }

    impl MetadataClient for Canned {
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            self.asked.lock().unwrap().push(url.to_owned());
            Ok(self.body.map(<[u8]>::to_vec))
        }
    }

    #[test]
    fn parses_a_real_single_disc_response() {
        let releases = parse(GUANO_APES, "TqvKjMu7dMliSfmVEBtrL7sBSno-").unwrap();
        assert_eq!(releases.len(), 1);
        let release = &releases[0];
        assert_eq!(release.mbid, "07090529-0fbf-4bd3-adc4-fe627343976d");
        assert_eq!(release.title, "Don\u{2019}t Give Me Names");
        assert_eq!(release.artist, "Guano Apes");
        assert_eq!(release.year, Some(2000));
        assert_eq!((release.disc_number, release.disc_count), (1, 1));
        assert_eq!(
            release.release_group_mbid.as_deref(),
            Some("9a3641db-6324-3db1-a406-075a2336e8bb")
        );
        assert_eq!(
            release.artist_mbid.as_deref(),
            Some("66da25f9-1534-4dd1-b88c-718bc24e1ccd")
        );
        assert_eq!(
            release.cover_art_url.as_deref(),
            Some(
                "https://coverartarchive.org/release/07090529-0fbf-4bd3-adc4-fe627343976d/front-500"
            )
        );

        assert_eq!(release.tracks.len(), 15);
        let first = &release.tracks[0];
        assert_eq!(
            (first.position, first.title.as_str(), first.artist.as_str()),
            (1, "Innocent Greed", "Guano Apes")
        );
        assert_eq!(
            first.recording_mbid.as_deref(),
            Some("95042c0a-5b64-4f61-ba75-2a1be26466d3")
        );
        assert_eq!(
            first.track_mbid.as_deref(),
            Some("2d691f20-c454-3b08-a533-803c32221fac")
        );
        assert_eq!(first.length_ms, Some(231_466));
        assert_eq!(release.tracks[5].title, "D\u{f6}del Up");
    }

    #[test]
    fn picks_the_medium_that_holds_the_disc_id() {
        let second = parse(TWO_DISCS, "uDAm7lKUREWYqiy5SA5BJ.OI6QM-").unwrap();
        assert_eq!(second.len(), 3);
        for release in &second {
            assert_eq!(
                (release.disc_number, release.disc_count),
                (2, 2),
                "{}",
                release.mbid
            );
            assert_eq!(release.tracks.len(), 13);
        }
        assert_eq!(second[0].tracks[0].title, "Hey You");
        assert_eq!(second[0].year, Some(1994));
        assert_eq!(second[0].artist, "Pink Floyd");

        let first = parse(TWO_DISCS, "P44Npk3QpAa1uQ25YVly2_YSQwU-").unwrap();
        assert_eq!(first[0].disc_number, 1);
        assert_ne!(first[0].tracks[0].title, second[0].tracks[0].title);
    }

    #[test]
    fn releases_without_the_disc_are_dropped() {
        assert!(
            parse(TWO_DISCS, "not-a-disc-in-this-response")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn missing_art_date_and_track_credits_degrade_gracefully() {
        let releases = parse(TWO_DISCS, "uDAm7lKUREWYqiy5SA5BJ.OI6QM-").unwrap();
        let bare = &releases[2];
        assert_eq!(bare.mbid, "00000000-0000-4000-8000-000000000000");
        assert_eq!(bare.year, None);
        assert_eq!(bare.cover_art_url, None);
        // This release's tracks carry no credit of their own; the recording's is used.
        assert!(bare.tracks.iter().all(|track| track.artist == "Pink Floyd"));
    }

    #[test]
    fn a_release_credit_with_a_join_phrase_is_concatenated() {
        let json = br#"{"releases":[{"id":"r","title":"T","artist-credit":[
            {"name":"Simon","joinphrase":" & ","artist":{"id":"a1"}},{"name":"Garfunkel","joinphrase":"","artist":{"id":"a2"}}],
            "media":[{"position":1,"discs":[{"id":"D"}],"tracks":[{"position":1,"title":"Song"}]}]}]}"#;
        let release = &parse(json, "D").unwrap()[0];
        assert_eq!(release.artist, "Simon & Garfunkel");
        assert_eq!(release.artist_mbid.as_deref(), Some("a1"));
        assert_eq!(release.tracks[0].artist, "Simon & Garfunkel");
        assert_eq!(release.tracks[0].recording_mbid, None);
    }

    #[test]
    fn an_id_that_is_not_a_uuid_gets_no_cover_url() {
        let json =
            br#"{"releases":[{"id":"x/../evil?","title":"T","cover-art-archive":{"front":true},
            "media":[{"discs":[{"id":"D"}],"tracks":[]}]}]}"#;
        assert_eq!(parse(json, "D").unwrap()[0].cover_art_url, None);
    }

    #[test]
    fn year_comes_from_any_date_precision() {
        assert_eq!(year_of("1997"), Some(1997));
        assert_eq!(year_of("1997-03"), Some(1997));
        assert_eq!(year_of("1997-03-04"), Some(1997));
        assert_eq!(year_of(""), None);
        assert_eq!(year_of("n/a"), None);
    }

    #[test]
    fn garbage_is_a_response_error() {
        assert!(matches!(
            parse(b"<html>", "D"),
            Err(Error::Response {
                service: "MusicBrainz",
                ..
            })
        ));
    }

    #[test]
    fn lookup_asks_for_recordings_credits_and_release_groups_by_disc_id() {
        let client = Canned {
            body: Some(GUANO_APES),
            asked: Mutex::default(),
        };
        let releases = lookup(&disc("TqvKjMu7dMliSfmVEBtrL7sBSno-"), &client).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(
            client.asked.lock().unwrap()[0],
            "https://musicbrainz.org/ws/2/discid/TqvKjMu7dMliSfmVEBtrL7sBSno-?inc=recordings+artist-credits+release-groups&fmt=json"
        );
    }

    #[test]
    fn an_unknown_disc_is_an_empty_list() {
        let client = Canned {
            body: None,
            asked: Mutex::default(),
        };
        assert!(lookup(&disc("unknown"), &client).unwrap().is_empty());
    }
}
