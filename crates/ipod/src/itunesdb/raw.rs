//! A verbatim copy of an existing iTunesDB, for rewriting it. The reader's `IpodTrack` models only
//! what the app shows; a rewrite built from it would silently drop what it does not model (gapless
//! data, volume adjustment, artwork links), so tracks and playlists keep their header bytes and
//! their mhods exactly as the iPod had them. Only what a sync changes is edited, and what is
//! derived from it (album and artist lists, sort indexes, lengths) is rebuilt by `emit`.

use std::collections::HashSet;

use super::chunks::{ListHead, MHLA, MHLI, MHLP, MHLT};
use super::mhod::{self, Mhod};
use super::{MHSD_TYPE_AT, database_chunk, mhit, playlists, sections};
use crate::bytes::{put, u8_at, u16_at, u32_at, u64_at};
use crate::error::{Error, Result};

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

impl RawDb {
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
