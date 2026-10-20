//! Serialising a [`RawDb`] back into an iTunesDB, following libgpod's write functions in
//! itdb_itunesdb.c (`write_mhsd_tracks`, `write_playlist`, `mk_mhia`, `mk_mhii`, `mk_mhip`).
//! Records that a sync did not touch are written back exactly as read; lengths, counts, album and
//! artist ids, the album and artist lists, the master playlist's sort indexes and the checksum are
//! recomputed because adding or removing a track invalidates them.

use std::collections::HashMap;

use super::mhit::at;
use super::mhod::{self, u32_from};
use super::raw::{Heads, RawDb, RawPlaylist, RawTrack, Slot};
use super::view::TrackView;
use super::{hash58, index};
use crate::bytes::put;
use crate::error::Result;

const MHIA_HEADER_LEN: usize = 88;
const MHII_HEADER_LEN: usize = 80;
const MHIP_HEADER_LEN: usize = 76;
const MHIP_COUNT_AT: usize = 12;
const MHIP_GROUP_FLAG_AT: usize = 16;
const MHIP_GROUP_ID_AT: usize = 20;
const MHIP_TRACK_ID_AT: usize = 24;
const MHIP_GROUP_REF_AT: usize = 32;
/// Marks an mhip that heads a podcast group instead of naming a track.
const GROUP_HEADER_FLAG: u32 = 256;
const MHYP_MHOD_COUNT_AT: usize = 12;
const MHYP_MHIP_COUNT_AT: usize = 16;
const PODCAST_SECTION: u32 = 3;

/// The output buffer. Length fields are patched by offset once a record is complete.
struct Out(Vec<u8>);

impl Out {
    fn begin(&mut self, header: &[u8]) -> usize {
        let start = self.0.len();
        self.0.extend_from_slice(header);
        start
    }

    fn append(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// Records the length of everything written since `start` in the chunk's `total_len`.
    fn finish(&mut self, start: usize) {
        let length = u32_from(self.0.len() - start);
        self.set_u32(start + 8, length);
    }

    /// Only call this straight after [`Out::begin`]: the end of the buffer is then the end of the
    /// header, so a field the header is too short to hold is skipped instead of landing in a child.
    fn set_u32(&mut self, at: usize, value: u32) {
        put(&mut self.0, at, &value.to_le_bytes());
    }
}

pub(crate) fn emit(db: &RawDb, firewire_id: Option<&[u8; 8]>) -> Result<Vec<u8>> {
    let views = db
        .tracks
        .iter()
        .map(TrackView::of)
        .collect::<Result<Vec<_>>>()?;
    let ids = Ids::assign(&views);
    let positions: HashMap<u32, usize> = db
        .tracks
        .iter()
        .enumerate()
        .map(|(position, track)| (track.id(), position))
        .collect();
    let next_group_id = db.tracks.iter().map(RawTrack::id).max().unwrap_or(0) + 1;
    let context = Context {
        db,
        views: &views,
        ids: &ids,
        positions: &positions,
    };

    let mut out = Out(Vec::new());
    let root = out.begin(&db.header);
    let mut group_id = next_group_id;
    for slot in &db.layout {
        match slot {
            Slot::Tracks(heads) => write_tracks(&mut out, heads, &context),
            Slot::Playlists { kind, heads } => {
                write_playlists(
                    &mut out,
                    heads,
                    *kind == PODCAST_SECTION,
                    &context,
                    &mut group_id,
                );
            }
            Slot::Albums(heads) => write_albums(&mut out, heads, &context),
            Slot::Artists(heads) => write_artists(&mut out, heads, &context),
            Slot::Opaque(bytes) => out.append(bytes),
        }
    }
    out.finish(root);

    let mut file = out.0;
    if let Some(firewire_id) = firewire_id {
        hash58::seal(&mut file, firewire_id)?;
    }
    Ok(file)
}

struct Context<'a> {
    db: &'a RawDb,
    views: &'a [TrackView],
    ids: &'a Ids,
    /// Track id to position in the track list.
    positions: &'a HashMap<u32, usize>,
}

/// libgpod's `prepare_itdb_for_write`: albums, artists and composers are numbered from 1 in track
/// order; 0 means the track has none.
pub(super) struct Ids {
    pub(super) album: Vec<u32>,
    pub(super) artist: Vec<u32>,
    composer: Vec<u32>,
    /// The first track of each album and artist, which supplies the list entry's strings.
    albums: Vec<usize>,
    artists: Vec<usize>,
}

impl Ids {
    pub(super) fn assign(views: &[TrackView]) -> Self {
        let mut ids = Self {
            album: Vec::new(),
            artist: Vec::new(),
            composer: Vec::new(),
            albums: Vec::new(),
            artists: Vec::new(),
        };
        let mut albums: HashMap<&str, Vec<u32>> = HashMap::new();
        let mut artists: HashMap<&str, u32> = HashMap::new();
        let mut composers: HashMap<&str, u32> = HashMap::new();
        for (position, view) in views.iter().enumerate() {
            let album = if view.album.is_empty() {
                0
            } else {
                let candidates = albums.entry(&view.album).or_default();
                let same = candidates
                    .iter()
                    .find(|&&id| same_album(&views[ids.albums[id as usize - 1]], view));
                match same {
                    Some(&id) => id,
                    None => {
                        ids.albums.push(position);
                        let id = u32_from(ids.albums.len());
                        candidates.push(id);
                        id
                    }
                }
            };
            ids.album.push(album);

            let artist = if view.artist.is_empty() {
                0
            } else {
                *artists.entry(&view.artist).or_insert_with(|| {
                    ids.artists.push(position);
                    u32_from(ids.artists.len())
                })
            };
            ids.artist.push(artist);

            let composer = if view.composer.is_empty() {
                0
            } else {
                let next = u32_from(composers.len() + 1);
                *composers.entry(&view.composer).or_insert(next)
            };
            ids.composer.push(composer);
        }
        ids
    }
}

/// libgpod's `itdb_album_equal`: same show and album, and the same album artist, or the same
/// artist when either track has no album artist.
fn same_album(a: &TrackView, b: &TrackView) -> bool {
    a.tv_show == b.tv_show
        && a.album == b.album
        && if !a.album_artist.is_empty() && !b.album_artist.is_empty() {
            a.album_artist == b.album_artist
        } else {
            a.artist == b.artist
        }
}

fn write_tracks(out: &mut Out, heads: &Heads, context: &Context) {
    let section = out.begin(&heads.mhsd);
    let list = out.begin(&heads.list);
    out.set_u32(list + 8, u32_from(context.db.tracks.len()));
    for (position, track) in context.db.tracks.iter().enumerate() {
        let start = out.begin(&track.header);
        out.set_u32(start + at::MHOD_COUNT, u32_from(track.mhods.len()));
        out.set_u32(start + at::ALBUM_ID, context.ids.album[position]);
        out.set_u32(start + at::ARTIST_ID, context.ids.artist[position]);
        out.set_u32(start + at::COMPOSER_ID, context.ids.composer[position]);
        for mhod in &track.mhods {
            out.append(&mhod.bytes);
        }
        out.finish(start);
    }
    out.finish(section);
}

fn write_albums(out: &mut Out, heads: &Heads, context: &Context) {
    let section = out.begin(&heads.mhsd);
    let list = out.begin(&heads.list);
    out.set_u32(list + 8, u32_from(context.ids.albums.len()));
    for (index, &position) in context.ids.albums.iter().enumerate() {
        let view = &context.views[position];
        let album_artist = if view.album_artist.is_empty() {
            &view.artist
        } else {
            &view.album_artist
        };
        let sort_artist = if view.sort_album_artist.is_empty() {
            &view.sort_artist
        } else {
            &view.sort_album_artist
        };
        let mhods: Vec<Vec<u8>> = [
            (mhod::LIST_ALBUM, &view.album),
            (mhod::LIST_ALBUM_ARTIST, album_artist),
            (mhod::LIST_ALBUM_SORT_ARTIST, sort_artist),
        ]
        .into_iter()
        .filter(|(_, text)| !text.is_empty())
        .map(|(kind, text)| mhod::encode_text(kind, text))
        .collect();
        // Padding words after the ids are zero except one that iTunes sets to 2.
        let key = [view.album.as_str(), album_artist, view.tv_show.as_str()];
        write_list_item(out, b"mhia", MHIA_HEADER_LEN, index, &key, &mhods);
    }
    out.finish(section);
}

fn write_artists(out: &mut Out, heads: &Heads, context: &Context) {
    let section = out.begin(&heads.mhsd);
    let list = out.begin(&heads.list);
    out.set_u32(list + 8, u32_from(context.ids.artists.len()));
    for (index, &position) in context.ids.artists.iter().enumerate() {
        let artist = &context.views[position].artist;
        let mhods = [mhod::encode_text(mhod::LIST_ARTIST, artist)];
        write_list_item(out, b"mhii", MHII_HEADER_LEN, index, &[artist], &mhods);
    }
    out.finish(section);
}

/// An mhia or mhii: child count, the 1-based id, a 64-bit id the SQLite databases of newer iPods
/// use (derived from the strings so rewrites stay stable), and the constant 2 at +28.
fn write_list_item(
    out: &mut Out,
    tag: &[u8; 4],
    header_len: usize,
    index: usize,
    key: &[&str],
    mhods: &[Vec<u8>],
) {
    let mut header = vec![0; header_len];
    put(&mut header, 0, tag);
    put(&mut header, 4, &u32_from(header_len).to_le_bytes());
    put(&mut header, 12, &u32_from(mhods.len()).to_le_bytes());
    put(&mut header, 16, &u32_from(index + 1).to_le_bytes());
    put(&mut header, 20, &stable_id(key).to_le_bytes());
    put(&mut header, 28, &2u32.to_le_bytes());
    let start = out.begin(&header);
    for mhod in mhods {
        out.append(mhod);
    }
    out.finish(start);
}

/// FNV-1a over the strings; never 0.
fn stable_id(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        for byte in part.bytes().chain([0]) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash | 1
}

fn write_playlists(
    out: &mut Out,
    heads: &Heads,
    grouped_podcasts: bool,
    context: &Context,
    next_group_id: &mut u32,
) {
    let section = out.begin(&heads.mhsd);
    let list = out.begin(&heads.list);
    out.set_u32(list + 8, u32_from(context.db.playlists.len()));
    for playlist in &context.db.playlists {
        write_playlist(out, playlist, grouped_podcasts, context, next_group_id);
    }
    out.finish(section);
}

fn write_playlist(
    out: &mut Out,
    playlist: &RawPlaylist,
    grouped_podcasts: bool,
    context: &Context,
    next_group_id: &mut u32,
) {
    // A member the track list no longer holds would make the iPod show an empty row.
    let members: Vec<usize> = playlist
        .members
        .iter()
        .filter_map(|id| context.positions.get(id).copied())
        .collect();
    let indexes = if playlist.is_master() && !members.is_empty() {
        let views: Vec<&TrackView> = members
            .iter()
            .map(|&position| &context.views[position])
            .collect();
        index::mhods(&views)
    } else {
        Vec::new()
    };
    let grouped = grouped_podcasts && playlist.is_podcasts();
    let groups = if grouped {
        group_by_album(&members, context.views)
    } else {
        Vec::new()
    };

    let start = out.begin(&playlist.header);
    out.set_u32(
        start + MHYP_MHOD_COUNT_AT,
        u32_from(playlist.mhods.len() + indexes.len()),
    );
    out.set_u32(
        start + MHYP_MHIP_COUNT_AT,
        u32_from(members.len() + groups.len()),
    );
    for mhod in &playlist.mhods {
        out.append(&mhod.bytes);
    }
    for mhod in &indexes {
        out.append(mhod);
    }

    if grouped {
        for (album, group) in groups {
            let group_id = take(next_group_id);
            write_entry(out, Entry::GroupHeader { group_id }, Some(&album));
            for position in group {
                let entry_id = take(next_group_id);
                let track_id = context.db.tracks[position].id();
                write_entry(
                    out,
                    Entry::Grouped {
                        track_id,
                        entry_id,
                        group_id,
                    },
                    None,
                );
            }
        }
    } else {
        for (index, &position) in members.iter().enumerate() {
            let track_id = context.db.tracks[position].id();
            write_entry(
                out,
                Entry::Plain {
                    track_id,
                    position: u32_from(index),
                },
                None,
            );
        }
    }
    out.finish(start);
}

fn take(counter: &mut u32) -> u32 {
    let value = *counter;
    *counter += 1;
    value
}

/// Members of a podcast playlist by album, in order of first appearance.
fn group_by_album(members: &[usize], views: &[TrackView]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for &position in members {
        let album = &views[position].album;
        match groups.iter_mut().find(|(name, _)| name == album) {
            Some((_, group)) => group.push(position),
            None => groups.push((album.clone(), vec![position])),
        }
    }
    groups
}

enum Entry {
    Plain {
        track_id: u32,
        position: u32,
    },
    GroupHeader {
        group_id: u32,
    },
    Grouped {
        track_id: u32,
        entry_id: u32,
        group_id: u32,
    },
}

/// An mhip and its single child mhod (libgpod's `mk_mhip`). iTunes 4.9 and later count the mhod as
/// a child, so the mhip's length covers it.
fn write_entry(out: &mut Out, entry: Entry, group_title: Option<&str>) {
    let mut header = vec![0; MHIP_HEADER_LEN];
    put(&mut header, 0, b"mhip");
    put(&mut header, 4, &u32_from(MHIP_HEADER_LEN).to_le_bytes());
    put(&mut header, MHIP_COUNT_AT, &1u32.to_le_bytes());
    let child = match entry {
        Entry::Plain { track_id, position } => {
            put(&mut header, MHIP_TRACK_ID_AT, &track_id.to_le_bytes());
            position_mhod(position)
        }
        Entry::GroupHeader { group_id } => {
            put(
                &mut header,
                MHIP_GROUP_FLAG_AT,
                &GROUP_HEADER_FLAG.to_le_bytes(),
            );
            put(&mut header, MHIP_GROUP_ID_AT, &group_id.to_le_bytes());
            mhod::encode_text(mhod::TITLE, group_title.unwrap_or_default())
        }
        Entry::Grouped {
            track_id,
            entry_id,
            group_id,
        } => {
            put(&mut header, MHIP_GROUP_ID_AT, &entry_id.to_le_bytes());
            put(&mut header, MHIP_TRACK_ID_AT, &track_id.to_le_bytes());
            put(&mut header, MHIP_GROUP_REF_AT, &group_id.to_le_bytes());
            position_mhod(entry_id)
        }
    };
    let start = out.begin(&header);
    out.append(&child);
    out.finish(start);
}

fn position_mhod(position: u32) -> Vec<u8> {
    let mut mhod = mhod::header(mhod::POSITION, 20);
    mhod.extend_from_slice(&position.to_le_bytes());
    mhod.extend_from_slice(&[0; 16]);
    mhod
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::super::edit::{Codec, Metadata, Origin};
    use super::super::raw::RawPlaylist;
    use super::*;
    use crate::bytes::{u32_at, u64_at};
    use crate::database::IpodMediaKind;

    fn header(tag: &[u8; 4], len: usize) -> Vec<u8> {
        let mut header = vec![0; len];
        put(&mut header, 0, tag);
        put(&mut header, 4, &u32_from(len).to_le_bytes());
        header
    }

    fn heads(section_type: u32, list_tag: &[u8; 4]) -> Heads {
        let mut mhsd = header(b"mhsd", 0x60);
        put(&mut mhsd, 12, &section_type.to_le_bytes());
        Heads {
            mhsd,
            list: header(list_tag, 0x5c),
        }
    }

    /// The expected hash was computed on a Mac in America/New_York, where 1970-01-01 (the dates
    /// `minimal_db` records) falls in standard time, UTC-5; the zone in force in October is not the
    /// one that applies to that date.
    fn pinned_zone() -> FixedOffset {
        FixedOffset::west_opt(5 * 3600).unwrap()
    }

    /// One track in a master playlist: the smallest database an iPod would accept.
    fn minimal_db() -> RawDb {
        let mut database = header(b"mhbd", 0xbc);
        put(&mut database, 16, &0x30u32.to_le_bytes());
        put(&mut database, 20, &2u32.to_le_bytes());
        put(&mut database, 0x18, &0x1122_3344_5566_7788u64.to_le_bytes());
        put(&mut database, 0x20, &2u16.to_le_bytes());

        let metadata = Metadata {
            title: "Song".into(),
            artist: "Artist".into(),
            album_artist: String::new(),
            album: "Album".into(),
            genre: String::new(),
            composer: String::new(),
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
            year: Some(2001),
            track_number: Some(1),
            track_count: None,
            disc_number: None,
            disc_count: None,
            rating: 0,
            compilation: false,
            sound_check: 0,
            play_count: 0,
            last_played: None,
        };
        let origin = Origin {
            id: 52,
            dbid: 0xaabb_ccdd_eeff_0011,
            ipod_path: ":iPod_Control:Music:F00:ABCD.mp3".into(),
            codec: Codec::Mp3,
            file_size: 1000,
            duration_ms: 1000,
            bitrate_kbps: Some(128),
            sample_rate: Some(44_100),
            date_added: 0,
            media_kind: IpodMediaKind::Music,
            database_tag: 0,
            extension: "mp3".into(),
        };
        let mut master = RawPlaylist::create(1, "Pod", vec![52], 0, &pinned_zone());
        put(&mut master.header, 20, &[1]);
        RawDb {
            header: database,
            layout: vec![
                Slot::Tracks(heads(1, b"mhlt")),
                Slot::Playlists {
                    kind: 2,
                    heads: heads(2, b"mhlp"),
                },
            ],
            tracks: vec![RawTrack::create(0x184, &origin, &metadata, &pinned_zone())],
            playlists: vec![master],
        }
    }

    #[test]
    fn the_minimal_database_is_written_and_read_back() {
        let file = emit(&minimal_db(), None).unwrap();

        let mount = std::path::Path::new("/ipod");
        let (tracks, playlists) = super::super::parse(&file, mount).unwrap();
        assert_eq!(u32_at(&file, 8), Some(u32_from(file.len())));
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Song");
        assert_eq!(
            tracks[0].path,
            mount.join("iPod_Control/Music/F00/ABCD.mp3")
        );
        assert_eq!(playlists.len(), 1);
        assert!(playlists[0].is_master);
        assert_eq!(playlists[0].track_ids, [52]);
    }

    #[test]
    fn sealing_makes_the_hash_the_one_libgpods_algorithm_computes() {
        let guid = [0x00, 0x0a, 0x27, 0x00, 0x1a, 0x2b, 0x3c, 0x4d];

        let file = emit(&minimal_db(), Some(&guid)).unwrap();

        assert_eq!(&file[0x30..0x32], &[1, 0], "scheme 1 is HASH58");
        assert_eq!(
            u64_at(&file, 0x18),
            Some(0x1122_3344_5566_7788),
            "the database id is untouched"
        );
        let hash: String = file[0x58..0x6c]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        // Computed by libgpod's itdb_hash58_write_hash (compiled from itdb_hash58.c, SHA-1 from
        // CommonCrypto) over exactly these bytes. If the layout of `minimal_db` output changes on
        // purpose, recompute it the same way.
        assert_eq!(hash, "ab3103056431196275965a5e33a0bc78be8f8232");
    }
}
