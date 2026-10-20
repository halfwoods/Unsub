use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use antistreaming_ipod::{
    Database, Error, IpodMediaKind, SyncPlan, SyncPlaylist, SyncTrack, apply_sync, plan_sync,
    read_database,
};

use crate::common::db::{self, Database as DbSpec, Playlist, Track};
use crate::common::mount::FakeIpod;
use crate::{classic_7g, opened, sample_database, sample_playlists, sample_tracks, unix_local};

pub const DB_FILE: &str = "iPod_Control/iTunes/iTunesDB";

pub fn itunes_file(fake: &FakeIpod, name: &str) -> PathBuf {
    fake.mount.join("iPod_Control/iTunes").join(name)
}

/// The library database, whichever of the two formats the fake iPod uses.
pub fn db_bytes(fake: &FakeIpod) -> Vec<u8> {
    fs::read(fake.mount.join(DB_FILE))
        .or_else(|_| fs::read(itunes_file(fake, "iTunesCDB")))
        .unwrap()
}

/// Every file under the music folders, to prove what a failed sync left behind.
pub fn music_files(fake: &FakeIpod) -> BTreeSet<PathBuf> {
    fn walk(dir: &Path, found: &mut BTreeSet<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.insert(path);
            }
        }
    }
    let mut found = BTreeSet::new();
    walk(&fake.mount.join("iPod_Control/Music"), &mut found);
    found
}

/// Files the app wrote next to the database, other than the database itself.
pub fn itunes_dir_listing(fake: &FakeIpod) -> BTreeSet<String> {
    fs::read_dir(fake.mount.join("iPod_Control/iTunes"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// A classic with the sample foreign library (as iTunes left it) and three music folders.
pub fn fake_classic() -> FakeIpod {
    let fake = classic_7g(&sample_database());
    for folder in ["F01", "F02"] {
        fs::create_dir_all(fake.mount.join("iPod_Control/Music").join(folder)).unwrap();
    }
    fake
}

pub fn library_track(library: &Path, library_id: i64, title: &str) -> SyncTrack {
    let source_path = library.join(format!("{library_id}.mp3"));
    fs::write(&source_path, vec![library_id as u8; 4000]).unwrap();
    SyncTrack {
        source_path,
        title: title.to_owned(),
        artist: "Björk".to_owned(),
        album_artist: "Björk".to_owned(),
        album: "Homogenic".to_owned(),
        genre: "Electronic".to_owned(),
        composer: "Guðmundur".to_owned(),
        year: Some(1997),
        track_number: Some(library_id as u32),
        track_count: Some(10),
        disc_number: Some(1),
        disc_count: Some(1),
        duration_ms: 245_000,
        bitrate_kbps: Some(256),
        sample_rate: Some(44_100),
        rating: 60,
        play_count: 4,
        last_played: Some(unix_local(2011, 3, 4, 12)),
        date_added: unix_local(2010, 1, 2, 12),
        media_kind: IpodMediaKind::Music,
        compilation: false,
        sort_title: Some(format!("sort {title}")),
        sort_artist: Some("Bjork".to_owned()),
        sort_album: None,
        sort_album_artist: Some("Bjork".to_owned()),
        artwork_key: None,
        library_id,
    }
}

/// Where the reader says the track titled `title` lives on the iPod.
pub fn track_path(fake: &FakeIpod, title: &str) -> PathBuf {
    read_database(&opened(fake))
        .unwrap()
        .tracks
        .iter()
        .find(|track| track.title == title)
        .unwrap_or_else(|| panic!("no track {title}"))
        .path
        .clone()
}

thread_local! {
    static COVERS: RefCell<HashMap<i64, Vec<u8>>> = RefCell::new(HashMap::new());
    static ASKED_FOR: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

/// Gives the track a cover. The library hands `apply_sync` only a cheap key per track and the bytes
/// on request, so the bytes live here (per test thread) and the key is a hash of them.
pub fn register_cover(track: &mut SyncTrack, cover: Vec<u8>) {
    let key = cover.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    track.artwork_key = Some(key);
    COVERS.with(|covers| covers.borrow_mut().insert(track.library_id, cover));
}

/// The `cover` callback of `apply_sync`, answering from [`register_cover`] and noting each request.
pub fn cover_provider(library_id: i64) -> Option<Vec<u8>> {
    ASKED_FOR.with(|asked| asked.borrow_mut().push(library_id));
    COVERS.with(|covers| covers.borrow().get(&library_id).cloned())
}

/// The library ids whose covers have been requested so far in this test, then forgotten.
pub fn covers_asked_for() -> Vec<i64> {
    ASKED_FOR.with(|asked| std::mem::take(&mut *asked.borrow_mut()))
}

pub fn no_cover(_: i64) -> Option<Vec<u8>> {
    None
}

pub fn copy_as_is(track: &SyncTrack) -> io::Result<PathBuf> {
    Ok(track.source_path.clone())
}

pub fn plan(fake: &FakeIpod, tracks: &[SyncTrack], playlists: &[SyncPlaylist]) -> SyncPlan {
    let ipod = opened(fake);
    let current = read_database(&ipod).unwrap();
    plan_sync(&ipod, &current, tracks, playlists).unwrap()
}

/// Plans and applies in one go, returning the plan and the database as written.
pub fn sync(
    fake: &FakeIpod,
    tracks: &[SyncTrack],
    playlists: &[SyncPlaylist],
) -> (SyncPlan, Database) {
    let plan = plan(fake, tracks, playlists);
    let database = apply_sync(
        &opened(fake),
        &plan,
        tracks,
        playlists,
        &mut copy_as_is,
        &mut cover_provider,
        &mut |_| {},
    )
    .unwrap();
    (plan, database)
}

mod add;
pub mod artwork;
mod faults;
mod incremental;
mod refusals;
mod structure;
mod swap;
