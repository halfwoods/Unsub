//! Reading an iPod to copy its music out of it, for a caller that puts the songs in a library.
//!
//! Nothing here writes to the iPod. It reads the database, `Unsub.json`, the On-The-Go files and
//! the music files, and nothing else, because the iPod may hold the only copy of the music.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use super::files::{has_protected_audio, music_folders};
use super::mapping::Mapping;
use super::on_the_go;
use crate::database::{IpodPlaylist, IpodTrack, read_database};
use crate::device::{Ipod, database_exists};
use crate::error::{Error, Result};

/// What an iPod hands a rescue.
#[derive(Debug, Clone, PartialEq)]
pub struct Rescue {
    /// Every song in the database, in its order.
    pub songs: Vec<RescueSong>,
    /// The iPod's playlists apart from the master, the podcasts list and the ones this app made,
    /// which mirror the library. Playlists made on the iPod itself come last.
    pub playlists: Vec<RescuePlaylist>,
    /// The iPod has no database (an iPod shuffle managed by something other than iTunes may keep
    /// only an iTunesSD), so the songs are the files in its music folders and carry nothing but
    /// their path and size: their details are for the caller to read from the files' own tags.
    pub from_files: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RescueSong {
    pub track: IpodTrack,
    /// The library id this app gave the song when it synced it to the iPod, so the library
    /// already has it.
    pub library_id: Option<i64>,
    /// The size of the file on the iPod, which the database can misstate; `None` if it isn't there.
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescuePlaylist {
    /// Identifies the playlist across reads: the iPod's own id, or `otg<n>` for a playlist made
    /// on the iPod.
    pub key: String,
    pub name: String,
    pub on_the_go: bool,
    pub smart: bool,
    /// `IpodTrack::id`s in playlist order.
    pub track_ids: Vec<u32>,
}

/// Reads the iPod for a rescue. Songs this app synced are marked by their library id; the rest
/// came from iTunes or another tool.
pub fn read_rescue(ipod: &Ipod) -> Result<Rescue> {
    let mount = ipod.mount_path.as_path();
    let mapping = Mapping::load(mount)?;
    if !database_exists(ipod) {
        return Ok(Rescue {
            songs: songs_in_music_folders(mount),
            playlists: Vec::new(),
            from_files: true,
        });
    }
    let database = read_database(ipod)?;
    let library_id_of = mapping.library_id_by_dbid();
    let songs = database
        .tracks
        .iter()
        .map(|track| RescueSong {
            library_id: library_id_of.get(&track.dbid).copied(),
            bytes: file_size(&track.path),
            track: track.clone(),
        })
        .collect();

    let ours: HashSet<u64> = mapping
        .playlists
        .iter()
        .map(|playlist| playlist.id)
        .collect();
    let mut playlists: Vec<RescuePlaylist> = database
        .playlists
        .iter()
        .filter(|playlist| {
            !playlist.is_master && !playlist.is_podcasts && !ours.contains(&playlist.id)
        })
        .map(playlist_of)
        .collect();
    // Playlists are an extra to the songs, so a damaged On-The-Go file costs only them.
    for (number, places) in on_the_go::listed(mount, database.tracks.len()).unwrap_or_default() {
        playlists.push(RescuePlaylist {
            key: format!("otg{number}"),
            name: format!("On-The-Go {number}"),
            on_the_go: true,
            smart: false,
            track_ids: places
                .into_iter()
                .filter_map(|place| Some(database.tracks.get(place)?.id))
                .collect(),
        });
    }
    Ok(Rescue {
        songs,
        playlists,
        from_files: false,
    })
}

fn playlist_of(playlist: &IpodPlaylist) -> RescuePlaylist {
    RescuePlaylist {
        key: playlist.id.to_string(),
        name: playlist.name.clone(),
        on_the_go: false,
        smart: playlist.is_smart,
        track_ids: playlist.track_ids.clone(),
    }
}

/// What iPods play, plus the protected books and songs they hold, so those are counted rather than
/// missed.
const MUSIC_EXTENSIONS: [&str; 10] = [
    "mp3", "m4a", "m4b", "m4p", "aa", "aax", "aac", "wav", "aif", "aiff",
];

fn songs_in_music_folders(mount: &Path) -> Vec<RescueSong> {
    let mut paths = Vec::new();
    for folder in music_folders(mount).unwrap_or_default() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        let mut found: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                let hidden = path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'));
                !hidden
                    && extension_of(path)
                        .is_some_and(|extension| MUSIC_EXTENSIONS.contains(&extension.as_str()))
            })
            .collect();
        found.sort();
        paths.extend(found);
    }
    paths
        .into_iter()
        .zip(1..)
        .map(|(path, id)| {
            let bytes = file_size(&path);
            RescueSong {
                track: IpodTrack {
                    id,
                    file_size: bytes.unwrap_or(0),
                    path,
                    ..IpodTrack::default()
                },
                library_id: None,
                bytes,
            }
        })
        .collect()
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

/// Whether the song's name or type says it is copy-protected audio, which cannot be played anywhere
/// but where it was authorised. iTunes labelled what it sold `.m4p` and "Protected AAC audio file"
/// (a flash iPod stores only the extension), and Audible's books are `.aa` and `.aax`. Nothing is
/// opened: see [`holds_protected_audio`] for the files that give no sign.
pub fn is_copy_protected(track: &IpodTrack) -> bool {
    let file_type = track.file_type.trim().to_ascii_lowercase();
    matches!(
        extension_of(&track.path).as_deref(),
        Some("m4p" | "aa" | "aax")
    ) || file_type == "m4p"
        || file_type.contains("protected")
        || file_type.contains("audible")
}

/// Whether the file itself holds encrypted audio, by its sample description: a protected song
/// renamed `.m4a` gives no other sign. This opens the file (read-only), so it is for a song about
/// to be copied rather than for counting a whole iPod.
pub fn holds_protected_audio(track: &IpodTrack) -> bool {
    matches!(
        extension_of(&track.path).as_deref(),
        Some("m4a" | "m4b" | "mp4")
    ) && has_protected_audio(&track.path)
}

/// How many bytes the disk `folder` is on has free.
pub fn free_space(folder: &Path) -> Result<u64> {
    fs4::available_space(folder).map_err(|source| Error::io(folder, source))
}

/// Refuses a copy of `needed` bytes into `folder` that the disk it is on cannot hold.
pub fn ensure_room(needed: u64, folder: &Path) -> Result<()> {
    let available = free_space(folder)?;
    if needed > available {
        return Err(Error::NoRoom {
            folder: folder.to_path_buf(),
            needed,
            available,
        });
    }
    Ok(())
}
