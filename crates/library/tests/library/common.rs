use std::fs;
use std::path::{Path, PathBuf};

use antistreaming_library::{Library, MediaKind, Track, TrackQuery};
use rusqlite::Connection;
use tempfile::TempDir;

pub const FIXTURE_COUNT: u64 = 8;
pub const MUSIC_COUNT: usize = 6;

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Copies the audio fixtures so tests can add sidecar files or edit tags
/// without touching the committed files.
pub fn copy_fixtures() -> TempDir {
    let dir = TempDir::new().unwrap();
    for entry in fs::read_dir(fixtures_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext != "sh") {
            fs::copy(&path, dir.path().join(path.file_name().unwrap())).unwrap();
        }
    }
    dir
}

pub fn scan(library: &mut Library, dir: &Path) {
    let summary = library.add_paths(&[dir.to_path_buf()], |_| {}).unwrap();
    assert!(
        summary.failed.is_empty(),
        "unexpected failures: {:?}",
        summary.failed
    );
}

/// An in-memory library holding every fixture. The temp dir must outlive the
/// library for artwork lookups to find the files.
pub fn scanned_library() -> (Library, TempDir) {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    (library, dir)
}

pub fn all_kinds() -> TrackQuery {
    TrackQuery {
        kind: None,
        ..TrackQuery::default()
    }
}

pub fn titles(library: &Library, query: &TrackQuery) -> Vec<String> {
    library
        .tracks(query)
        .unwrap()
        .into_iter()
        .map(|track| track.title)
        .collect()
}

pub fn track_titled(library: &Library, title: &str) -> Track {
    library
        .tracks(&all_kinds())
        .unwrap()
        .into_iter()
        .find(|track| track.title == title)
        .unwrap_or_else(|| panic!("no track titled {title:?}"))
}

/// A synthetic row for tests that need exact control over the fields; use
/// struct update syntax to vary the rest.
pub fn track(id: i64, title: &str, artist: &str, album: &str, duration_ms: u64) -> Track {
    Track {
        id,
        path: format!("/music/{id}.mp3"),
        title: title.into(),
        artist: artist.into(),
        album_artist: artist.into(),
        album: album.into(),
        genre: String::new(),
        composer: String::new(),
        year: None,
        track_number: None,
        track_count: None,
        disc_number: None,
        disc_count: None,
        duration_ms,
        bitrate_kbps: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        codec: "MP3".into(),
        file_size: 1,
        date_added: 0,
        date_modified: 0,
        play_count: 0,
        last_played: None,
        skip_count: 0,
        rating: 0,
        kind: MediaKind::Music,
        compilation: false,
        enabled: true,
        in_cloud: false,
        missing: false,
        loudness: None,
        sort_title: None,
        sort_artist: None,
        sort_album: None,
        sort_album_artist: None,
    }
}

pub fn library_of(tracks: &[Track]) -> Library {
    let mut library = Library::open_in_memory().unwrap();
    library.import_tracks(tracks).unwrap();
    library
}

/// Turns a current-schema database file back into an older one, as if it had
/// been created by an older build, so migrations can be run on real data.
pub fn rewind_schema(db_path: &Path, version: u32) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "ALTER TABLE tracks DROP COLUMN loudness_lufs;
         ALTER TABLE tracks DROP COLUMN true_peak_dbtp;
         ALTER TABLE tracks DROP COLUMN loudness_source;
         ALTER TABLE tracks DROP COLUMN album_loudness_lufs;
         ALTER TABLE tracks DROP COLUMN album_true_peak_dbtp;
         ALTER TABLE tracks DROP COLUMN loudness_state;",
    )
    .unwrap();
    if version < 6 {
        conn.execute_batch(
            "DROP TABLE settings;
             ALTER TABLE tracks DROP COLUMN missing;",
        )
        .unwrap();
    }
    if version < 5 {
        conn.execute_batch("ALTER TABLE tracks DROP COLUMN in_cloud;")
            .unwrap();
    }
    if version < 4 {
        conn.execute_batch(
            "DROP TABLE play_events;
             ALTER TABLE tracks DROP COLUMN overrides;",
        )
        .unwrap();
    }
    if version < 3 {
        conn.execute_batch(
            "DROP TABLE playlist_items;
             DROP TABLE playlists;
             DROP INDEX tracks_artist_group;
             DROP INDEX tracks_album_group;
             ALTER TABLE tracks DROP COLUMN enabled;
             ALTER TABLE tracks DROP COLUMN artist_group;
             ALTER TABLE tracks DROP COLUMN album_group;",
        )
        .unwrap();
    }
    if version < 2 {
        conn.execute_batch("ALTER TABLE tracks DROP COLUMN parser_version;")
            .unwrap();
    }
    conn.pragma_update(None, "user_version", version).unwrap();
}
