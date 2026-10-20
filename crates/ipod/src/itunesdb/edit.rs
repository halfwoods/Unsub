//! Creating and editing track records the way libgpod's writer lays them out (`mk_mhit` and
//! `itdb_track_set_defaults` in itdb_itunesdb.c and itdb_track.c).

use super::mhit::at;
use super::mhod;
use super::raw::{RawMhod, RawPlaylist, RawTrack};
use crate::bytes::put;
use crate::database::IpodMediaKind;
use crate::time::unix_to_mac;

/// What a sync may change about a track that is already on the iPod.
pub(crate) struct Metadata {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album_artist: String,
    pub(crate) album: String,
    pub(crate) genre: String,
    pub(crate) composer: String,
    pub(crate) sort_title: Option<String>,
    pub(crate) sort_artist: Option<String>,
    pub(crate) sort_album: Option<String>,
    pub(crate) sort_album_artist: Option<String>,
    pub(crate) year: Option<i32>,
    pub(crate) track_number: Option<u32>,
    pub(crate) track_count: Option<u32>,
    pub(crate) disc_number: Option<u32>,
    pub(crate) disc_count: Option<u32>,
    pub(crate) rating: u8,
    pub(crate) compilation: bool,
    pub(crate) sound_check: u32,
    pub(crate) play_count: u32,
    pub(crate) last_played: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Codec {
    Mp3,
    Aac,
    Alac,
    Wav,
    Aiff,
    Other,
}

impl Codec {
    /// The type string iTunes stores in mhod 6.
    fn type_name(self) -> &'static str {
        match self {
            Self::Mp3 => "MPEG audio file",
            Self::Aac => "AAC audio file",
            Self::Alac => "Apple Lossless audio file",
            Self::Wav => "WAV audio file",
            Self::Aiff => "AIFF audio file",
            Self::Other => "",
        }
    }

    /// libgpod's `itdb_track_set_defaults` matches the type string against these families.
    fn is_lossy_family(self) -> bool {
        matches!(self, Self::Mp3 | Self::Aac)
    }
}

/// Everything about a track that comes from the file placed on the iPod rather than from tags.
pub(crate) struct Origin {
    pub(crate) id: u32,
    pub(crate) dbid: u64,
    /// `:iPod_Control:Music:F00:ABCD.mp3`
    pub(crate) ipod_path: String,
    pub(crate) codec: Codec,
    pub(crate) file_size: u64,
    pub(crate) duration_ms: u64,
    pub(crate) bitrate_kbps: Option<u32>,
    pub(crate) sample_rate: Option<u32>,
    pub(crate) date_added: i64,
    pub(crate) media_kind: IpodMediaKind,
    /// The mhbd's field at 0x24, which libgpod repeats in every mhit.
    pub(crate) database_tag: u64,
    /// Lower-case file extension without the dot.
    pub(crate) extension: String,
}

/// libgpod's `Itdb_Mediatype` bit for each kind. Video podcasts are not distinguished.
fn media_type(kind: IpodMediaKind) -> u32 {
    match kind {
        IpodMediaKind::Music | IpodMediaKind::Other => 0x01,
        IpodMediaKind::Video => 0x02,
        IpodMediaKind::Podcast => 0x04,
        IpodMediaKind::Audiobook => 0x08,
    }
}

/// `.mp3` becomes `MP3 `, packed the way libgpod's `itdb_cp_finalize` does.
fn file_type_marker(extension: &str) -> u32 {
    let mut marker = 0u32;
    let mut letters = extension.bytes().map(|byte| byte.to_ascii_uppercase());
    for _ in 0..4 {
        marker = (marker << 8) | u32::from(letters.next().unwrap_or(b' '));
    }
    marker
}

impl RawTrack {
    /// A new track record with a header of `header_len` bytes. Fields beyond a short header are
    /// left out, as older iTunes versions did not have them.
    pub(crate) fn create(header_len: usize, origin: &Origin, metadata: &Metadata) -> Self {
        let mut header = vec![0; header_len];
        put(&mut header, 0, b"mhit");
        put(&mut header, 4, &mhod::u32_from(header_len).to_le_bytes());
        let mut track = Self {
            header,
            mhods: Vec::new(),
        };

        let marker = file_type_marker(&origin.extension);
        let kind = origin.media_kind;
        let added = unix_to_mac(origin.date_added);
        let hz = origin.sample_rate.unwrap_or(0);
        track.set_u32(at::ID, origin.id);
        track.set_u32(at::VISIBLE, 1);
        track.set_u32(at::FILE_TYPE_MARKER, marker);
        track.set_u8(at::TYPE2, u8::from(origin.codec == Codec::Mp3));
        track.set_u32(at::TIME_MODIFIED, added);
        track.set_u32(
            at::FILE_SIZE,
            u32::try_from(origin.file_size).unwrap_or(u32::MAX),
        );
        track.set_u32(
            at::DURATION_MS,
            u32::try_from(origin.duration_ms).unwrap_or(u32::MAX),
        );
        track.set_u32(at::BITRATE, origin.bitrate_kbps.unwrap_or(0));
        track.set_u32(at::SAMPLE_RATE, hz << 16);
        track.set_u32(at::DATE_ADDED, added);
        put(&mut track.header, at::DBID, &origin.dbid.to_le_bytes());
        put(
            &mut track.header,
            at::DBID_REPEAT,
            &origin.dbid.to_le_bytes(),
        );
        track.set_u16(
            at::UNKNOWN_126,
            if origin.codec.is_lossy_family() {
                0xffff
            } else {
                0
            },
        );
        put(
            &mut track.header,
            at::SAMPLE_RATE_FLOAT,
            &(hz as f32).to_le_bytes(),
        );
        let unknown_144: u16 = match origin.codec {
            Codec::Mp3 => 0x0c,
            Codec::Aac => 0x33,
            _ => 0,
        };
        put(
            &mut track.header,
            at::UNKNOWN_144,
            &unknown_144.to_le_bytes(),
        );
        // 2 means "no artwork"; artwork is not written yet.
        track.set_u8(at::HAS_ARTWORK, 2);
        let spoken_word = matches!(kind, IpodMediaKind::Podcast | IpodMediaKind::Audiobook);
        track.set_u8(at::SKIP_WHEN_SHUFFLING, u8::from(spoken_word));
        track.set_u8(
            at::REMEMBER_POSITION,
            u8::from(kind == IpodMediaKind::Audiobook),
        );
        track.set_u8(at::MOVIE_FLAG, u8::from(kind == IpodMediaKind::Video));
        // 2 shows the bullet that marks an unplayed podcast.
        let unplayed = kind == IpodMediaKind::Podcast && metadata.play_count == 0;
        track.set_u8(at::MARK_UNPLAYED, if unplayed { 2 } else { 1 });
        track.set_u32(at::MEDIA_TYPE, media_type(kind));
        put(
            &mut track.header,
            at::DATABASE_TAG,
            &origin.database_tag.to_le_bytes(),
        );
        track.set_u32(
            at::FILE_SIZE_REPEAT,
            u32::try_from(origin.file_size).unwrap_or(u32::MAX),
        );
        put(
            &mut track.header,
            at::UNKNOWN_0X134,
            &0x8080_8080_8080u64.to_le_bytes(),
        );
        track.set_u32(at::ARTWORK_LINK_FLAG, 1);

        track.set_text(mhod::FILE_TYPE, Some(origin.codec.type_name()));
        track.set_text(mhod::PATH, Some(&origin.ipod_path));
        track.apply(metadata);
        track
    }

    /// Points the track at its image in the ArtworkDB (libgpod's `itdb_track_set_thumbnails`).
    pub(crate) fn link_artwork(&mut self, image_id: u32, source_len: u32) {
        self.set_u16(at::ARTWORK_COUNT, 1);
        self.set_u32(at::ARTWORK_SIZE, source_len);
        self.set_u8(at::HAS_ARTWORK, 1);
        self.set_u32(at::MHII_LINK, image_id);
    }

    /// The image this track links to, 0 for none.
    pub(crate) fn artwork_link(&self) -> u32 {
        crate::bytes::u32_at(&self.header, at::MHII_LINK).unwrap_or(0)
    }

    /// Writes the editable fields; everything else in the record is left as it was.
    pub(crate) fn apply(&mut self, metadata: &Metadata) {
        self.set_text(mhod::TITLE, Some(&metadata.title));
        self.set_text(mhod::ARTIST, Some(&metadata.artist));
        self.set_text(mhod::ALBUM, Some(&metadata.album));
        self.set_text(mhod::GENRE, Some(&metadata.genre));
        self.set_text(mhod::COMPOSER, Some(&metadata.composer));
        self.set_text(mhod::ALBUM_ARTIST, Some(&metadata.album_artist));
        self.set_text(mhod::SORT_ARTIST, metadata.sort_artist.as_deref());
        self.set_text(mhod::SORT_TITLE, metadata.sort_title.as_deref());
        self.set_text(mhod::SORT_ALBUM, metadata.sort_album.as_deref());
        self.set_text(
            mhod::SORT_ALBUM_ARTIST,
            metadata.sort_album_artist.as_deref(),
        );

        let number = |value: Option<u32>| value.unwrap_or(0);
        self.set_u32(
            at::YEAR,
            metadata
                .year
                .and_then(|year| u32::try_from(year).ok())
                .unwrap_or(0),
        );
        self.set_u32(at::TRACK_NUMBER, number(metadata.track_number));
        self.set_u32(at::TRACK_COUNT, number(metadata.track_count));
        self.set_u32(at::DISC_NUMBER, number(metadata.disc_number));
        self.set_u32(at::DISC_COUNT, number(metadata.disc_count));
        self.set_u8(at::RATING, metadata.rating);
        self.set_u8(at::APP_RATING, metadata.rating);
        self.set_u8(at::COMPILATION, u8::from(metadata.compilation));
        self.set_u32(at::SOUND_CHECK, metadata.sound_check);
        self.set_u32(at::PLAY_COUNT, metadata.play_count);
        self.set_u32(at::PLAY_COUNT_REPEAT, metadata.play_count);
        self.set_u32(at::LAST_PLAYED, metadata.last_played.map_or(0, unix_to_mac));
    }
}

/// The header libgpod writes for a playlist (`write_playlist`).
const PLAYLIST_HEADER_LEN: usize = 108;
const SORT_ORDER_MANUAL: u32 = 1;

impl RawPlaylist {
    pub(crate) fn create(id: u64, name: &str, members: Vec<u32>, now_unix: i64) -> Self {
        let mut header = vec![0; PLAYLIST_HEADER_LEN];
        put(&mut header, 0, b"mhyp");
        put(
            &mut header,
            4,
            &mhod::u32_from(PLAYLIST_HEADER_LEN).to_le_bytes(),
        );
        put(&mut header, 24, &unix_to_mac(now_unix).to_le_bytes());
        put(&mut header, 28, &id.to_le_bytes());
        // One string mhod (the title), then the sort order iTunes uses for hand-built playlists.
        put(&mut header, 40, &1u16.to_le_bytes());
        put(&mut header, 44, &SORT_ORDER_MANUAL.to_le_bytes());
        Self {
            header,
            mhods: vec![
                RawMhod {
                    kind: mhod::TITLE,
                    bytes: mhod::encode_text(mhod::TITLE, name),
                },
                RawMhod {
                    kind: mhod::POSITION,
                    bytes: view_preferences(),
                },
            ],
            members,
        }
    }
}

/// The 648-byte column and view preferences mhod that every iTunes playlist carries. libgpod
/// hard-codes it ("seems to be an itunespref thing"); without it iTunes shows the playlist empty.
fn view_preferences() -> Vec<u8> {
    // (value, number of zero words after it)
    const WORDS: [(u32, usize); 14] = [
        (0x01_0084, 0),
        (0x05, 0),
        (0x09, 0),
        (0x03, 0),
        (0x12_0001, 3),
        (0xc8_0002, 3),
        (0x3c_000d, 3),
        (0x7d_0004, 3),
        (0x7d_0003, 3),
        (0x64_0008, 3),
        (0x64_0017, 0),
        (0x01, 2),
        (0x50_0014, 0),
        (0x01, 2),
    ];
    const TAIL: [(u32, usize); 2] = [(0x7d_0015, 0), (0x01, 114)];

    let mut mhod = mhod::header(mhod::POSITION, 648 - 24);
    // Bytes 16..40: the 8 unknown header bytes written by `header` plus 16 zero bytes.
    mhod.extend_from_slice(&[0; 16]);
    for (value, zeros) in WORDS.into_iter().chain(TAIL) {
        mhod.extend_from_slice(&value.to_le_bytes());
        mhod.extend(std::iter::repeat_n(0u8, 4 * zeros));
    }
    mhod
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::{u8_at, u16_at, u32_at, u64_at};

    fn metadata() -> Metadata {
        Metadata {
            title: "Title".into(),
            artist: "Artist".into(),
            album_artist: String::new(),
            album: "Album".into(),
            genre: "Genre".into(),
            composer: String::new(),
            sort_title: None,
            sort_artist: Some("Sort Artist".into()),
            sort_album: None,
            sort_album_artist: None,
            year: Some(1999),
            track_number: Some(3),
            track_count: None,
            disc_number: None,
            disc_count: None,
            rating: 80,
            compilation: true,
            sound_check: 0,
            play_count: 0,
            last_played: None,
        }
    }

    fn origin(codec: Codec, kind: IpodMediaKind) -> Origin {
        Origin {
            id: 77,
            dbid: 0x1122_3344_5566_7788,
            ipod_path: ":iPod_Control:Music:F01:ABCD.mp3".into(),
            codec,
            file_size: 4_000_000,
            duration_ms: 200_000,
            bitrate_kbps: Some(256),
            sample_rate: Some(44_100),
            date_added: 1_300_000_000,
            media_kind: kind,
            database_tag: 0xdead_beef,
            extension: "mp3".into(),
        }
    }

    #[test]
    fn the_view_preferences_mhod_has_the_length_libgpod_declares() {
        let mhod = view_preferences();

        assert_eq!(mhod.len(), 0x288);
        assert_eq!(u32_at(&mhod, 8), Some(0x288));
        assert_eq!(u32_at(&mhod, 12), Some(100));
        assert_eq!(u32_at(&mhod, 40), Some(0x01_0084));
        assert_eq!(u32_at(&mhod, 184), Some(0x7d_0015));
    }

    #[test]
    fn a_new_playlist_has_a_title_and_view_preferences() {
        let playlist = RawPlaylist::create(0xabc, "Road Trip", vec![3, 1], 1_300_000_000);

        assert_eq!(playlist.id(), 0xabc);
        assert_eq!(playlist.name().unwrap(), "Road Trip");
        assert!(!playlist.is_master() && !playlist.is_podcasts() && !playlist.is_smart());
        assert_eq!(playlist.members, [3, 1]);
        assert_eq!(playlist.header.len(), 108);
    }

    #[test]
    fn packs_the_extension_into_a_space_padded_four_character_code() {
        assert_eq!(file_type_marker("mp3").to_be_bytes(), *b"MP3 ");
        assert_eq!(file_type_marker("m4a").to_be_bytes(), *b"M4A ");
        assert_eq!(file_type_marker("flac2").to_be_bytes(), *b"FLAC");
        assert_eq!(file_type_marker("").to_be_bytes(), *b"    ");
    }

    #[test]
    fn a_full_size_header_carries_the_libgpod_defaults() {
        let track = RawTrack::create(
            0x248,
            &origin(Codec::Mp3, IpodMediaKind::Music),
            &metadata(),
        );

        let header = &track.header;
        assert_eq!(header.len(), 0x248);
        assert_eq!(u32_at(header, at::ID), Some(77));
        assert_eq!(u32_at(header, at::VISIBLE), Some(1));
        assert_eq!(u8_at(header, at::TYPE2), Some(1));
        assert_eq!(u32_at(header, at::SAMPLE_RATE), Some(44_100 << 16));
        assert_eq!(u64_at(header, at::DBID), Some(0x1122_3344_5566_7788));
        assert_eq!(u64_at(header, at::DBID_REPEAT), Some(0x1122_3344_5566_7788));
        assert_eq!(u16_at(header, at::UNKNOWN_126), Some(0xffff));
        assert_eq!(u32_at(header, at::MEDIA_TYPE), Some(1));
        assert_eq!(u8_at(header, at::MARK_UNPLAYED), Some(1));
        assert_eq!(u64_at(header, at::UNKNOWN_0X134), Some(0x8080_8080_8080));
        assert_eq!(u32_at(header, at::YEAR), Some(1999));
        assert_eq!(u8_at(header, at::RATING), Some(80));
        assert_eq!(
            track.text(mhod::FILE_TYPE).unwrap().as_deref(),
            Some("MPEG audio file")
        );
        assert_eq!(
            track.text(mhod::SORT_ARTIST).unwrap().as_deref(),
            Some("Sort Artist")
        );
        assert_eq!(track.text(mhod::COMPOSER).unwrap(), None);
    }

    #[test]
    fn a_short_header_simply_lacks_the_newer_fields() {
        let track = RawTrack::create(0x9c, &origin(Codec::Aac, IpodMediaKind::Music), &metadata());

        assert_eq!(track.header.len(), 0x9c);
        assert_eq!(u32_at(&track.header, at::BITRATE), Some(256));
        assert_eq!(u32_at(&track.header, at::MEDIA_TYPE), None);
    }

    #[test]
    fn unplayed_podcasts_get_the_bullet_and_skip_shuffle() {
        let track = RawTrack::create(
            0x184,
            &origin(Codec::Mp3, IpodMediaKind::Podcast),
            &metadata(),
        );

        assert_eq!(u8_at(&track.header, at::MARK_UNPLAYED), Some(2));
        assert_eq!(u8_at(&track.header, at::SKIP_WHEN_SHUFFLING), Some(1));
        assert_eq!(u32_at(&track.header, at::MEDIA_TYPE), Some(4));
    }

    #[test]
    fn applying_metadata_replaces_and_removes_strings() {
        let mut track = RawTrack::create(
            0x184,
            &origin(Codec::Mp3, IpodMediaKind::Music),
            &metadata(),
        );
        let mut changed = metadata();
        changed.title = "New".into();
        changed.sort_artist = None;
        changed.year = None;

        track.apply(&changed);

        assert_eq!(track.text(mhod::TITLE).unwrap().as_deref(), Some("New"));
        assert_eq!(track.text(mhod::SORT_ARTIST).unwrap(), None);
        assert_eq!(u32_at(&track.header, at::YEAR), Some(0));
        assert_eq!(
            track.text(mhod::PATH).unwrap().as_deref(),
            Some(":iPod_Control:Music:F01:ABCD.mp3")
        );
    }
}
