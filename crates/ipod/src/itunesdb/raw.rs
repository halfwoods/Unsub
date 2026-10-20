//! A verbatim copy of an existing iTunesDB, for rewriting it. The reader's `IpodTrack` models only
//! what the app shows; a rewrite built from it would silently drop what it does not model (gapless
//! data, volume adjustment, artwork links), so tracks and playlists keep their header bytes and
//! their mhods exactly as the iPod had them. Only what a sync changes is edited, and what is
//! derived from it (album and artist lists, sort indexes, lengths) is rebuilt by `emit`.

use std::collections::HashSet;

use super::chunks::{ListHead, MHLA, MHLI, MHLP, MHLT};
use super::mhod::{self, Mhod};
use super::{MHBD_SECTION_COUNT_AT, MHSD_TYPE_AT, database_chunk, mhit, playlists, sections};
use crate::bytes::{put, u8_at, u16_at, u32_at, u64_at};
use crate::error::{Error, Result};

/// The header sizes libgpod's writer uses for dbversion 0x30 (iTunes 9.2), which the tracks a sync
/// creates follow too.
const MHBD_HEADER_LEN: usize = 244;
const MHSD_HEADER_LEN: usize = 0x60;
const LIST_HEADER_LEN: usize = 0x5c;
const DB_VERSION: u32 = 0x30;
/// Offsets in the mhbd header of fields a new database fills in (`MhbdHeader` in libgpod).
const LANGUAGE_AT: usize = 0x46;
const PERSISTENT_ID_AT: usize = 0x48;
const UTC_OFFSET_AT: usize = 0x6c;
/// The two letters of the language code, in file order. libgpod's `itdb_new` stores the number
/// 0x656e little-endian, which puts them the other way round.
const LANGUAGE_EN: [u8; 2] = *b"en";
const MIN_TRACK_HEADER_LEN: usize = 0x9c;
const MIN_PLAYLIST_HEADER_LEN: usize = 48;
const SECTION_TRACKS: u32 = 1;
const SECTION_PLAYLISTS: u32 = 2;
const SECTION_PODCAST_PLAYLISTS: u32 = 3;
const SECTION_ALBUMS: u32 = 4;
const SECTION_ARTISTS: u32 = 8;

mod at {
    pub(crate) const TRACK_ID: usize = 16;
    pub(crate) const TRACK_DBID: usize = 112;
    pub(crate) const PLAYLIST_TYPE: usize = 20;
    pub(crate) const PLAYLIST_ID: usize = 28;
    pub(crate) const PLAYLIST_PODCAST_FLAG: usize = 42;
}

/// Playlist `type` byte 1 marks the master playlist; `podcastflag` 1 the Podcasts playlist.
const MASTER_TYPE: u8 = 1;
const PODCASTS_FLAG: u16 = 1;

pub(crate) struct RawMhod {
    pub(crate) kind: u32,
    pub(crate) bytes: Vec<u8>,
}

impl RawMhod {
    pub(crate) fn text(&self) -> Result<String> {
        Mhod::read(&self.bytes, 0, self.bytes.len())?.text()
    }
}

pub(crate) struct RawTrack {
    pub(crate) header: Vec<u8>,
    pub(crate) mhods: Vec<RawMhod>,
}

impl RawTrack {
    pub(crate) fn id(&self) -> u32 {
        u32_at(&self.header, at::TRACK_ID).unwrap_or(0)
    }

    pub(crate) fn dbid(&self) -> u64 {
        u64_at(&self.header, at::TRACK_DBID).unwrap_or(0)
    }

    /// The last mhod of this type wins, as in the reader.
    pub(crate) fn text(&self, kind: u32) -> Result<Option<String>> {
        let Some(mhod) = self.mhods.iter().rev().find(|mhod| mhod.kind == kind) else {
            return Ok(None);
        };
        Ok(Some(mhod.text()?).filter(|text| !text.is_empty()))
    }

    /// Replaces every mhod of `kind`; `None` or an empty string removes it.
    pub(crate) fn set_text(&mut self, kind: u32, text: Option<&str>) {
        self.mhods.retain(|mhod| mhod.kind != kind);
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.mhods.push(RawMhod {
                kind,
                bytes: mhod::encode_text(kind, text),
            });
        }
    }

    pub(crate) fn set_u8(&mut self, at: usize, value: u8) {
        put(&mut self.header, at, &[value]);
    }

    pub(crate) fn set_u16(&mut self, at: usize, value: u16) {
        put(&mut self.header, at, &value.to_le_bytes());
    }

    pub(crate) fn set_u32(&mut self, at: usize, value: u32) {
        put(&mut self.header, at, &value.to_le_bytes());
    }
}

pub(crate) struct RawPlaylist {
    pub(crate) header: Vec<u8>,
    /// Every mhod except the sort indexes (types 52 and 53), which are derived from the tracks.
    pub(crate) mhods: Vec<RawMhod>,
    /// Member track ids in playlist order.
    pub(crate) members: Vec<u32>,
}

impl RawPlaylist {
    pub(crate) fn id(&self) -> u64 {
        u64_at(&self.header, at::PLAYLIST_ID).unwrap_or(0)
    }

    pub(crate) fn is_master(&self) -> bool {
        u8_at(&self.header, at::PLAYLIST_TYPE) == Some(MASTER_TYPE)
    }

    pub(crate) fn is_podcasts(&self) -> bool {
        u16_at(&self.header, at::PLAYLIST_PODCAST_FLAG) == Some(PODCASTS_FLAG)
    }

    /// This playlist marked as the master playlist, the one that lists every track.
    pub(crate) fn into_master(mut self) -> Self {
        put(&mut self.header, at::PLAYLIST_TYPE, &[MASTER_TYPE]);
        self
    }

    pub(crate) fn is_smart(&self) -> bool {
        self.mhods
            .iter()
            .any(|mhod| matches!(mhod.kind, mhod::SMART_PREFS | mhod::SMART_RULES))
    }

    /// iTunes sometimes writes two titles; the last one wins, as in libgpod.
    pub(crate) fn name(&self) -> Result<String> {
        self.mhods
            .iter()
            .rev()
            .find(|mhod| mhod.kind == mhod::TITLE)
            .map_or_else(|| Ok(String::new()), RawMhod::text)
    }
}

/// The two header blocks of a section: the mhsd and the list (mhlt, mhlp, mhla, mhli) inside it.
pub(crate) struct Heads {
    pub(crate) mhsd: Vec<u8>,
    pub(crate) list: Vec<u8>,
}

/// What each mhsd of the original file becomes, in the original order.
pub(crate) enum Slot {
    Tracks(Heads),
    /// Type 2 holds the playlists as they are; type 3 holds the same playlists with the Podcasts
    /// playlist grouped by show.
    Playlists {
        kind: u32,
        heads: Heads,
    },
    Albums(Heads),
    Artists(Heads),
    /// Smart-playlist categories, Genius data and anything unknown: not derived from the track
    /// list, so copied untouched.
    Opaque(Vec<u8>),
}

pub(crate) struct RawDb {
    pub(crate) header: Vec<u8>,
    pub(crate) layout: Vec<Slot>,
    pub(crate) tracks: Vec<RawTrack>,
    pub(crate) playlists: Vec<RawPlaylist>,
}

/// A chunk header of `len` bytes that is all zeros after its tag and length.
fn header(tag: &[u8; 4], len: usize) -> Vec<u8> {
    let mut header = vec![0; len];
    put(&mut header, 0, tag);
    put(&mut header, 4, &mhod::u32_from(len).to_le_bytes());
    header
}

impl RawDb {
    /// A database with no tracks and only the master playlist, for an iPod that has none yet. It
    /// is filled in the way libgpod's `itdb_new` and `mk_mhbd` do: the sections of the database
    /// libgpod wrote for `tests/fixtures` (tracks, podcast playlists, playlists) plus the album
    /// list, a random database id and persistent id, the language `en`, and the zone's offset
    /// from UTC in seconds. libgpod leaves the unknown id at 0x24 at zero in a
    /// new database, and so does this.
    pub(crate) fn blank(
        db_id: u64,
        persistent_id: u64,
        utc_offset_secs: i32,
        master: RawPlaylist,
    ) -> Self {
        let section = |kind: u32, list: &[u8; 4]| {
            let mut mhsd = header(b"mhsd", MHSD_HEADER_LEN);
            put(&mut mhsd, MHSD_TYPE_AT, &kind.to_le_bytes());
            Heads {
                mhsd,
                list: header(list, LIST_HEADER_LEN),
            }
        };
        let layout = vec![
            Slot::Tracks(section(SECTION_TRACKS, b"mhlt")),
            Slot::Playlists {
                kind: SECTION_PODCAST_PLAYLISTS,
                heads: section(SECTION_PODCAST_PLAYLISTS, b"mhlp"),
            },
            Slot::Playlists {
                kind: SECTION_PLAYLISTS,
                heads: section(SECTION_PLAYLISTS, b"mhlp"),
            },
            Slot::Albums(section(SECTION_ALBUMS, b"mhla")),
        ];
        let mut mhbd = header(b"mhbd", MHBD_HEADER_LEN);
        put(&mut mhbd, 12, &1u32.to_le_bytes());
        put(&mut mhbd, 16, &DB_VERSION.to_le_bytes());
        put(
            &mut mhbd,
            MHBD_SECTION_COUNT_AT,
            &mhod::u32_from(layout.len()).to_le_bytes(),
        );
        put(&mut mhbd, 24, &db_id.to_le_bytes());
        // 1 for a Mac, as libgpod's `itdb_new` says; the Windows value 2 is not needed.
        put(&mut mhbd, 32, &1u16.to_le_bytes());
        put(&mut mhbd, LANGUAGE_AT, &LANGUAGE_EN);
        put(&mut mhbd, PERSISTENT_ID_AT, &persistent_id.to_le_bytes());
        put(&mut mhbd, UTC_OFFSET_AT, &utc_offset_secs.to_le_bytes());
        Self {
            header: mhbd,
            layout,
            tracks: Vec::new(),
            playlists: vec![master],
        }
    }

    pub(crate) fn parse(data: &[u8]) -> Result<Self> {
        let database = database_chunk(data)?;
        let mut db = Self {
            header: database.header().to_vec(),
            layout: Vec::new(),
            tracks: Vec::new(),
            playlists: Vec::new(),
        };
        let mut seen = HashSet::new();
        for section in sections(data, &database)? {
            let kind = section.u32(MHSD_TYPE_AT).unwrap_or(0);
            let first_of_kind = seen.insert(kind);
            let heads = |list: &ListHead| Heads {
                mhsd: section.header().to_vec(),
                list: list.header(data).to_vec(),
            };
            let slot = match kind {
                SECTION_TRACKS if first_of_kind => {
                    let list = ListHead::find(data, &section, MHLT)?;
                    db.tracks = mhit::parse_raw_list(data, &section, &list)?;
                    Slot::Tracks(heads(&list))
                }
                SECTION_PLAYLISTS if first_of_kind => {
                    db.playlists = playlists::parse_raw_list(data, &section)?;
                    Slot::Playlists {
                        kind,
                        heads: heads(&ListHead::find(data, &section, MHLP)?),
                    }
                }
                SECTION_PODCAST_PLAYLISTS if first_of_kind => Slot::Playlists {
                    kind,
                    heads: heads(&ListHead::find(data, &section, MHLP)?),
                },
                SECTION_ALBUMS if first_of_kind => {
                    Slot::Albums(heads(&ListHead::find(data, &section, MHLA)?))
                }
                SECTION_ARTISTS if first_of_kind => {
                    Slot::Artists(heads(&ListHead::find(data, &section, MHLI)?))
                }
                _ => Slot::Opaque(
                    data.get(section.offset..section.end)
                        .unwrap_or_default()
                        .to_vec(),
                ),
            };
            db.layout.push(slot);
        }

        // libgpod rejects records with shorter headers than these; rewriting one would shift fields.
        if db
            .tracks
            .iter()
            .any(|track| track.header.len() < MIN_TRACK_HEADER_LEN)
        {
            return Err(Error::corrupt(
                0,
                "a track record has a header too short to rewrite",
            ));
        }
        if db
            .playlists
            .iter()
            .any(|playlist| playlist.header.len() < MIN_PLAYLIST_HEADER_LEN)
        {
            return Err(Error::corrupt(
                0,
                "a playlist record has a header too short to rewrite",
            ));
        }
        // New tracks are listed through the master playlist; without one they would be invisible.
        if !db.playlists.iter().any(RawPlaylist::is_master) {
            return Err(Error::corrupt(0, "no master playlist"));
        }
        let has = |wanted: fn(&Slot) -> bool| db.layout.iter().any(wanted);
        if !has(|slot| matches!(slot, Slot::Tracks(_))) {
            return Err(Error::corrupt(0, "no track list section (mhsd type 1)"));
        }
        if !has(|slot| {
            matches!(
                slot,
                Slot::Playlists {
                    kind: SECTION_PLAYLISTS,
                    ..
                }
            )
        }) {
            return Err(Error::corrupt(
                0,
                "no standard playlist section (mhsd type 2)",
            ));
        }
        Ok(db)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::itunesdb::{emit, master_playlist_name};

    fn blank() -> Vec<u8> {
        let master = RawPlaylist::create(7, "Riley’s iPod", Vec::new(), 1_300_000_000, &Utc);
        emit(
            &RawDb::blank(9, 0x1122_3344_5566_7788, -18_000, master.into_master()),
            None,
        )
        .unwrap()
    }

    #[test]
    fn a_blank_database_is_one_the_reader_and_the_rewriter_accept() {
        let bytes = blank();

        let db = RawDb::parse(&bytes).unwrap();

        assert!(db.tracks.is_empty());
        assert_eq!(db.playlists.len(), 1);
        assert!(db.playlists[0].is_master() && !db.playlists[0].is_podcasts());
        assert_eq!(db.playlists[0].id(), 7);
        let kinds: Vec<_> = (db.layout.iter())
            .map(|slot| match slot {
                Slot::Tracks(_) => "tracks",
                Slot::Playlists { kind: 2, .. } => "playlists",
                Slot::Playlists { kind: 3, .. } => "podcasts",
                Slot::Albums(_) => "albums",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["tracks", "podcasts", "playlists", "albums"]);
        assert_eq!(
            master_playlist_name(&bytes).unwrap().as_deref(),
            Some("Riley’s iPod")
        );
        assert_eq!(
            emit(&db, None).unwrap(),
            bytes,
            "writing it again changes nothing"
        );
    }

    #[test]
    fn a_blank_database_counts_its_sections_and_carries_the_ids_it_was_given() {
        let bytes = blank();

        assert_eq!(u32_at(&bytes, MHBD_SECTION_COUNT_AT), Some(4));
        assert_eq!(u32_at(&bytes, 8), Some(bytes.len() as u32));
        assert_eq!(u64_at(&bytes, 24), Some(9));
        assert_eq!(u32_at(&bytes, 4), Some(244), "the header libgpod writes");
    }

    #[test]
    fn a_blank_database_says_english() {
        let bytes = blank();

        assert_eq!(&bytes[LANGUAGE_AT..LANGUAGE_AT + 2], b"en");
    }

    #[test]
    fn a_blank_database_has_the_persistent_id_it_was_given() {
        let bytes = blank();

        assert_eq!(
            u64_at(&bytes, PERSISTENT_ID_AT),
            Some(0x1122_3344_5566_7788)
        );
        assert_ne!(u64_at(&bytes, 24), u64_at(&bytes, PERSISTENT_ID_AT));
    }

    #[test]
    fn a_blank_database_records_the_offset_from_utc_in_seconds() {
        let bytes = blank();

        assert_eq!(
            i32::from_le_bytes(bytes[UTC_OFFSET_AT..UTC_OFFSET_AT + 4].try_into().unwrap()),
            -18_000
        );
    }

    #[test]
    fn the_unknown_id_at_0x24_stays_zero_as_in_libgpods_new_database() {
        let bytes = blank();

        assert_eq!(u64_at(&bytes, 0x24), Some(0));
    }
}
