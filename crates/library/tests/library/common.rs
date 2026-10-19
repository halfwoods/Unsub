use std::fs;
use std::path::{Path, PathBuf};

use antistreaming_library::{Library, Track, TrackQuery};
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
