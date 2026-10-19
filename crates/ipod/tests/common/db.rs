use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// Header sizes iTunes 9 wrote (dbversion 0x30).
const MHBD_HEADER: usize = 0xbc;
const MHSD_HEADER: usize = 0x60;
const MHLT_HEADER: usize = 0x5c;
const MHYP_HEADER: usize = 0x6c;
const MHIP_HEADER: usize = 0x4c;
const MHOD_HEADER: usize = 0x18;
/// iTunes 9 wrote 0x184 for mhit; older versions wrote 0x9c (4.7 and earlier) and 0xf4 (4.7.1).
pub const MHIT_HEADER: usize = 0x184;

pub const MHOD_TITLE: u32 = 1;
pub const MHOD_PATH: u32 = 2;
pub const MHOD_ALBUM: u32 = 3;
pub const MHOD_ARTIST: u32 = 4;
pub const MHOD_GENRE: u32 = 5;
pub const MHOD_FILE_TYPE: u32 = 6;
pub const MHOD_COMPOSER: u32 = 12;
pub const MHOD_PODCAST_URL: u32 = 15;
pub const MHOD_ALBUM_ARTIST: u32 = 22;
pub const MHOD_SORT_ARTIST: u32 = 23;
pub const MHOD_SORT_TITLE: u32 = 27;
pub const MHOD_SORT_ALBUM: u32 = 28;
pub const MHOD_SORT_ALBUM_ARTIST: u32 = 29;
pub const MHOD_SMART_PREFS: u32 = 50;
pub const MHOD_POSITION: u32 = 100;

pub const MEDIA_AUDIO: u32 = 0x01;
pub const MEDIA_VIDEO: u32 = 0x02;
pub const MEDIA_PODCAST: u32 = 0x04;
pub const MEDIA_AUDIOBOOK: u32 = 0x08;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf16,
    Utf8,
}

/// A record under construction: a zeroed header with typed setters, finished into bytes. A write
/// past the end of a short header is dropped, which is how fields added by newer iTunes versions
/// are absent from older files.
struct Record(Vec<u8>);

impl Record {
    fn new(tag: &[u8; 4], header_len: usize) -> Self {
        let mut bytes = vec![0; header_len];
        bytes[..4].copy_from_slice(tag);
        let mut record = Self(bytes);
        record.u32(4, u32::try_from(header_len).unwrap());
        record
    }

    fn put(&mut self, at: usize, bytes: &[u8]) -> &mut Self {
        if let Some(field) = self.0.get_mut(at..at + bytes.len()) {
            field.copy_from_slice(bytes);
        }
        self
    }

    fn u8(&mut self, at: usize, value: u8) -> &mut Self {
        self.put(at, &[value])
    }

    fn u16(&mut self, at: usize, value: u16) -> &mut Self {
        self.put(at, &value.to_le_bytes())
    }

    fn u32(&mut self, at: usize, value: u32) -> &mut Self {
        self.put(at, &value.to_le_bytes())
    }

    fn u64(&mut self, at: usize, value: u64) -> &mut Self {
        self.put(at, &value.to_le_bytes())
    }

    /// Appends `children` and records the total length.
    fn finish(&mut self, children: &[u8]) -> Vec<u8> {
        self.0.extend_from_slice(children);
        let total = u32::try_from(self.0.len()).unwrap();
        self.u32(8, total);
        std::mem::take(&mut self.0)
    }

    /// For mhlt and mhlp, whose third field is an item count and whose items follow as siblings.
    fn finish_list(&mut self, count: usize) -> Vec<u8> {
        self.u32(8, u32::try_from(count).unwrap());
        std::mem::take(&mut self.0)
    }
}

pub fn mhod_text(kind: u32, text: &str, encoding: Encoding) -> Vec<u8> {
    let (flag, data): (u32, Vec<u8>) = match encoding {
        Encoding::Utf16 => (1, text.encode_utf16().flat_map(u16::to_le_bytes).collect()),
        Encoding::Utf8 => (2, text.as_bytes().to_vec()),
    };
    let mut body = Vec::new();
    body.extend_from_slice(&flag.to_le_bytes());
    body.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&data);
    Record::new(b"mhod", MHOD_HEADER)
        .u32(12, kind)
        .finish(&body)
}

/// A podcast URL: raw UTF-8 straight after the header, with no encoding flag or length.
pub fn mhod_raw(kind: u32, payload: &[u8]) -> Vec<u8> {
    Record::new(b"mhod", MHOD_HEADER)
        .u32(12, kind)
        .finish(payload)
}

pub fn mhod_position(position: u32) -> Vec<u8> {
    let mut body = position.to_le_bytes().to_vec();
    body.extend_from_slice(&[0; 16]);
    Record::new(b"mhod", MHOD_HEADER)
        .u32(12, MHOD_POSITION)
        .finish(&body)
}

/// A smart-playlist preferences mhod; the reader only needs to notice it exists.
fn mhod_smart_prefs() -> Vec<u8> {
    Record::new(b"mhod", MHOD_HEADER)
        .u32(12, MHOD_SMART_PREFS)
        .finish(&[0; 72])
}

#[derive(Clone)]
pub struct Track {
    pub id: u32,
    pub dbid: u64,
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub composer: String,
    pub file_type: Option<String>,
    pub file_type_marker: u32,
    pub sort_title: Option<String>,
    pub sort_artist: Option<String>,
    pub sort_album: Option<String>,
    pub sort_album_artist: Option<String>,
    pub year: u32,
    pub track_number: u32,
    pub track_count: u32,
    pub disc_number: u32,
    pub disc_count: u32,
    pub duration_ms: u32,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub size: u32,
    pub play_count: u32,
    pub last_played_mac: u32,
    pub skip_count: u32,
    pub rating: u8,
    pub date_added_mac: u32,
    pub media_type: u32,
    pub movie_flag: bool,
    pub compilation: bool,
    pub encoding: Encoding,
    pub extra_mhods: Vec<Vec<u8>>,
}

impl Track {
    pub fn new(id: u32, title: &str) -> Self {
        Self {
            id,
            dbid: 0x0123_4567_89ab_cd00 + u64::from(id),
            path: format!(":iPod_Control:Music:F00:TRK{id:03}.mp3"),
            title: title.to_owned(),
            artist: String::new(),
            album: String::new(),
            album_artist: String::new(),
            genre: String::new(),
            composer: String::new(),
            file_type: Some("MPEG audio file".to_owned()),
            file_type_marker: 0,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
            year: 0,
            track_number: 0,
            track_count: 0,
            disc_number: 0,
            disc_count: 0,
            duration_ms: 180_000,
            bitrate: 256,
            sample_rate: 44_100,
            size: 5_000_000,
            play_count: 0,
            last_played_mac: 0,
            skip_count: 0,
            rating: 0,
            date_added_mac: 0,
            media_type: MEDIA_AUDIO,
            movie_flag: false,
            compilation: false,
            encoding: Encoding::Utf16,
            extra_mhods: Vec::new(),
        }
    }

    fn mhit(&self, header_len: usize) -> Vec<u8> {
        let mut mhods = Vec::new();
        let mut text = |kind: u32, value: &str| {
            if !value.is_empty() {
                mhods.push(mhod_text(kind, value, self.encoding));
            }
        };
        text(MHOD_TITLE, &self.title);
        text(MHOD_PATH, &self.path);
        text(MHOD_ALBUM, &self.album);
        text(MHOD_ARTIST, &self.artist);
        text(MHOD_GENRE, &self.genre);
        text(
            MHOD_FILE_TYPE,
            self.file_type.as_deref().unwrap_or_default(),
        );
        text(MHOD_COMPOSER, &self.composer);
        text(MHOD_ALBUM_ARTIST, &self.album_artist);
        text(
            MHOD_SORT_TITLE,
            self.sort_title.as_deref().unwrap_or_default(),
        );
        text(
            MHOD_SORT_ARTIST,
            self.sort_artist.as_deref().unwrap_or_default(),
        );
        text(
            MHOD_SORT_ALBUM,
            self.sort_album.as_deref().unwrap_or_default(),
        );
        text(
            MHOD_SORT_ALBUM_ARTIST,
            self.sort_album_artist.as_deref().unwrap_or_default(),
        );
        mhods.extend(self.extra_mhods.iter().cloned());

        let mut mhit = Record::new(b"mhit", header_len);
        mhit.u32(12, u32::try_from(mhods.len()).unwrap())
            .u32(16, self.id)
            .u32(24, self.file_type_marker)
            .u8(30, u8::from(self.compilation))
            .u8(31, self.rating)
            .u32(36, self.size)
            .u32(40, self.duration_ms)
            .u32(44, self.track_number)
            .u32(48, self.track_count)
            .u32(52, self.year)
            .u32(56, self.bitrate)
            .u32(60, self.sample_rate << 16)
            .u32(80, self.play_count)
            .u32(88, self.last_played_mac)
            .u32(92, self.disc_number)
            .u32(96, self.disc_count)
            .u32(104, self.date_added_mac)
            .u64(112, self.dbid)
            .u32(156, self.skip_count)
            .u8(177, u8::from(self.movie_flag))
            .u32(208, self.media_type);
        mhit.finish(&mhods.concat())
    }
}

#[derive(Clone)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub master: bool,
    pub podcasts: bool,
    pub smart: bool,
    /// `(track id, position)` in file order; the reader must order by position.
    pub members: Vec<(u32, u32)>,
    /// Write mhips the way iTunes 4.7 and earlier did: `total_len` equal to `header_len`, with the
    /// position mhod following as a sibling.
    pub legacy_mhips: bool,
}

impl Playlist {
    pub fn new(id: u64, name: &str, track_ids: &[u32]) -> Self {
        let members = track_ids
            .iter()
            .zip(0..)
            .map(|(&track, position)| (track, position))
            .collect();
        Self {
            id,
            name: name.to_owned(),
            master: false,
            podcasts: false,
            smart: false,
            members,
            legacy_mhips: false,
        }
    }

    pub fn master(id: u64, name: &str, track_ids: &[u32]) -> Self {
        Self {
            master: true,
            ..Self::new(id, name, track_ids)
        }
    }

    pub fn podcasts(id: u64, name: &str, track_ids: &[u32]) -> Self {
        Self {
            podcasts: true,
            ..Self::new(id, name, track_ids)
        }
    }

    /// `grouped` writes the type-3 section layout: one group header mhip before the members.
    fn mhyp(&self, grouped: bool) -> Vec<u8> {
        let mut mhods = vec![mhod_text(MHOD_TITLE, &self.name, Encoding::Utf16)];
        if self.smart {
            mhods.push(mhod_smart_prefs());
        }
        let mut mhips = Vec::new();
        let mhip_count = usize::from(grouped && self.podcasts) + self.members.len();
        if grouped && self.podcasts {
            let group_title = mhod_text(MHOD_TITLE, "Podcast show", Encoding::Utf16);
            mhips.push(
                Record::new(b"mhip", MHIP_HEADER)
                    .u32(12, 1)
                    .u32(16, 256)
                    .u32(20, 900)
                    .finish(&group_title),
            );
        }
        for &(track, position) in &self.members {
            let mut mhip = Record::new(b"mhip", MHIP_HEADER);
            mhip.u32(12, 1).u32(24, track);
            if self.legacy_mhips {
                mhips.push(mhip.finish(&[]));
                mhips.push(mhod_position(position));
            } else {
                mhips.push(mhip.finish(&mhod_position(position)));
            }
        }

        let mut mhyp = Record::new(b"mhyp", MHYP_HEADER);
        mhyp.u32(12, u32::try_from(mhods.len()).unwrap())
            .u32(16, u32::try_from(mhip_count).unwrap())
            .u8(20, u8::from(self.master))
            .u64(28, self.id)
            .u16(42, u16::from(self.podcasts));
        mhyp.finish(&[mhods.concat(), mhips.concat()].concat())
    }
}

pub struct Database {
    pub tracks: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub mhit_header: usize,
    /// Write the type-3 playlist section that iTunes adds for podcast-aware iPods.
    pub podcast_section: bool,
}

impl Database {
    pub fn new(tracks: Vec<Track>, playlists: Vec<Playlist>) -> Self {
        Self {
            tracks,
            playlists,
            mhit_header: MHIT_HEADER,
            podcast_section: true,
        }
    }

    fn sections(&self) -> Vec<Vec<u8>> {
        let mhits: Vec<Vec<u8>> = self
            .tracks
            .iter()
            .map(|track| track.mhit(self.mhit_header))
            .collect();
        let track_list = [
            Record::new(b"mhlt", MHLT_HEADER).finish_list(mhits.len()),
            mhits.concat(),
        ]
        .concat();

        let playlist_section = |kind: u32, grouped: bool| {
            let mhyps: Vec<Vec<u8>> = self
                .playlists
                .iter()
                .map(|playlist| playlist.mhyp(grouped))
                .collect();
            let list = [
                Record::new(b"mhlp", MHLT_HEADER).finish_list(mhyps.len()),
                mhyps.concat(),
            ]
            .concat();
            Record::new(b"mhsd", MHSD_HEADER)
                .u32(12, kind)
                .finish(&list)
        };

        let mut sections = vec![
            Record::new(b"mhsd", MHSD_HEADER)
                .u32(12, 1)
                .finish(&track_list),
        ];
        if self.podcast_section {
            sections.push(playlist_section(3, true));
        }
        sections.push(playlist_section(2, false));
        // Album list and smart-playlist sections: present in real files, ignored by the reader.
        sections.push(
            Record::new(b"mhsd", MHSD_HEADER)
                .u32(12, 4)
                .finish(&Record::new(b"mhla", MHLT_HEADER).finish_list(0)),
        );
        let hidden = Playlist::new(7, "IGNORED-TYPE-5", &[]).mhyp(false);
        let smart_list = [Record::new(b"mhlp", MHLT_HEADER).finish_list(1), hidden].concat();
        sections.push(
            Record::new(b"mhsd", MHSD_HEADER)
                .u32(12, 5)
                .finish(&smart_list),
        );
        sections
    }

    fn header(&self, section_count: usize) -> Record {
        let mut mhbd = Record::new(b"mhbd", MHBD_HEADER);
        mhbd.u32(16, 0x30)
            .u32(20, u32::try_from(section_count).unwrap())
            .u64(24, 0x1122_3344_5566_7788)
            .u16(32, 2);
        mhbd
    }

    pub fn build(&self) -> Vec<u8> {
        let sections = self.sections();
        self.header(sections.len()).finish(&sections.concat())
    }

    /// iTunesCDB: the mhbd header stays plain, flag 1 at 0xa8, and the rest is one zlib stream.
    pub fn build_cdb(&self) -> Vec<u8> {
        let sections = self.sections();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&sections.concat()).unwrap();
        let compressed = encoder.finish().unwrap();
        self.header(sections.len()).u8(0xa8, 1).finish(&compressed)
    }
}

/// A Play Counts file in the firmware 2.0+ layout: 0x1c-byte entries in iTunesDB track order.
pub struct PlayCount {
    pub plays: u32,
    pub last_played_mac: u32,
    pub rating: u32,
    pub skips: u32,
}

pub fn play_counts_file(entries: &[PlayCount]) -> Vec<u8> {
    let mut file = Record::new(b"mhdp", 0x60);
    file.u32(8, 0x1c)
        .u32(12, u32::try_from(entries.len()).unwrap());
    let mut bytes = std::mem::take(&mut file.0);
    for entry in entries {
        let mut record = [0u8; 0x1c];
        record[0..4].copy_from_slice(&entry.plays.to_le_bytes());
        record[4..8].copy_from_slice(&entry.last_played_mac.to_le_bytes());
        record[12..16].copy_from_slice(&entry.rating.to_le_bytes());
        record[20..24].copy_from_slice(&entry.skips.to_le_bytes());
        bytes.extend_from_slice(&record);
    }
    bytes
}
