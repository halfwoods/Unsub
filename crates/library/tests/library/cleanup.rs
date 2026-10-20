use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use lofty::file::TaggedFileExt;
use lofty::tag::Accessor;
use tempfile::TempDir;
use unsub_library::{
    Capitalization, CleanUpField, CleanUpRules, EditReport, Library, MediaKind, TrackEdits,
};

use crate::common::{all_kinds, scanned_library, track, track_titled};

fn all_rules() -> CleanUpRules {
    CleanUpRules {
        tidy: true,
        remove_junk: true,
        remove_track_numbers: true,
        remove_artist: true,
        ..CleanUpRules::default()
    }
}

fn clean_up(library: &mut Library, ids: &[i64], rules: &CleanUpRules) -> EditReport {
    library
        .clean_up(ids, rules, &AtomicBool::new(false), |_, _| {})
        .unwrap()
}

/// Halo has track 3 on disc 2 and Come Together has its numbers cleared, so one name's
/// numbers must be left alone and the other's filled in.
fn untidy_library() -> (Library, TempDir, [i64; 3]) {
    let (mut library, dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    let together = track_titled(&library, "Come Together").id;
    let something = track_titled(&library, "Something").id;
    let rename = |library: &mut Library, id: i64, edits: TrackEdits| {
        library.edit_tracks(&[id], &edits).unwrap();
    };
    rename(
        &mut library,
        halo,
        TrackEdits {
            title: Some("01 - beyonce - Halo [HD]".into()),
            ..TrackEdits::default()
        },
    );
    rename(
        &mut library,
        together,
        TrackEdits {
            title: Some("2-04_Come_Together".into()),
            track_number: Some(None),
            disc_number: Some(None),
            ..TrackEdits::default()
        },
    );
    rename(
        &mut library,
        something,
        TrackEdits {
            title: Some("Something (Official Video)".into()),
            ..TrackEdits::default()
        },
    );
    (library, dir, [halo, together, something])
}

fn file_tag_of(path: &str) -> (Option<String>, Option<u32>, Option<u32>) {
    let tagged = lofty::read_from_path(path).unwrap();
    let tag = tagged.primary_tag().unwrap();
    (tag.title().map(|t| t.to_string()), tag.track(), tag.disk())
}

#[test]
fn previewing_lists_the_changes_and_writes_nothing() {
    let (library, dir, [halo, together, something]) = untidy_library();
    let rows = library.tracks(&all_kinds()).unwrap();
    let files: Vec<_> = rows
        .iter()
        .map(|row| fs::read(&row.path).unwrap())
        .collect();
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();

    let changes = library.clean_up_preview(&ids, &all_rules()).unwrap();

    let summary: Vec<_> = changes
        .iter()
        .map(|c| (c.id, c.after.as_str(), c.track_number, c.disc_number))
        .collect();
    assert_eq!(changes.len(), 3, "{summary:?}");
    assert!(summary.contains(&(halo, "Halo", None, None)));
    assert!(summary.contains(&(together, "Come Together", Some(4), Some(2))));
    assert!(summary.contains(&(something, "Something", None, None)));
    assert_eq!(library.tracks(&all_kinds()).unwrap(), rows);
    let after: Vec<_> = rows
        .iter()
        .map(|row| fs::read(&row.path).unwrap())
        .collect();
    assert_eq!(after, files, "no file was touched");
    assert!(
        fs::read_dir(dir.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp"))
    );
}

#[test]
fn applying_changes_the_chosen_field_and_never_overwrites_a_track_number() {
    let (mut library, _dir, [halo, together, something]) = untidy_library();
    let ids = [halo, together, something];
    let before_halo = library.track(halo).unwrap().unwrap();

    let report = clean_up(&mut library, &ids, &all_rules());

    assert_eq!(report.written.len(), 3);
    assert!(report.db_only.is_empty(), "{:?}", report.db_only);
    let halo_row = library.track(halo).unwrap().unwrap();
    assert_eq!(halo_row.title, "Halo");
    assert_eq!(
        (halo_row.track_number, halo_row.disc_number),
        (before_halo.track_number, before_halo.disc_number),
        "the numbers Halo had are kept"
    );
    assert_eq!(
        halo_row.artist, before_halo.artist,
        "its own artist, not rewritten"
    );
    let together_row = library.track(together).unwrap().unwrap();
    assert_eq!(
        (
            together_row.title.as_str(),
            together_row.track_number,
            together_row.disc_number
        ),
        ("Come Together", Some(4), Some(2))
    );
    assert_eq!(
        library.track(something).unwrap().unwrap().title,
        "Something"
    );
    assert_eq!(
        file_tag_of(&together_row.path),
        (Some("Come Together".to_owned()), Some(4), Some(2)),
        "the file carries them too"
    );
    assert_eq!(
        file_tag_of(&halo_row.path),
        (Some("Halo".to_owned()), Some(3), Some(2))
    );
    assert!(
        library
            .clean_up_preview(&ids, &all_rules())
            .unwrap()
            .is_empty(),
        "a second pass has nothing left to do"
    );
}

#[test]
fn another_field_changes_alone() {
    let (mut library, _dir) = scanned_library();
    let zombie = track_titled(&library, "Zombie").id;
    library
        .edit_tracks(
            &[zombie],
            &TrackEdits {
                artist: Some("the cranberries".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    let rules = CleanUpRules {
        field: CleanUpField::Artist,
        capitalize: Some(Capitalization::Title),
        ..CleanUpRules::default()
    };

    let changes = library.clean_up_preview(&[zombie], &rules).unwrap();
    clean_up(&mut library, &[zombie], &rules);

    assert_eq!(changes[0].before, "the cranberries");
    let row = library.track(zombie).unwrap().unwrap();
    assert_eq!(
        (row.artist.as_str(), row.title.as_str()),
        ("The Cranberries", "Zombie")
    );
}

#[test]
fn a_name_that_would_end_up_empty_is_left_as_it_is() {
    let (mut library, _dir) = scanned_library();
    let halo = track_titled(&library, "Halo").id;
    library
        .edit_tracks(
            &[halo],
            &TrackEdits {
                title: Some("[HD]".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    let rules = CleanUpRules {
        remove_junk: true,
        ..CleanUpRules::default()
    };

    assert!(
        library
            .clean_up_preview(&[halo], &rules)
            .unwrap()
            .is_empty()
    );
    let report = clean_up(&mut library, &[halo], &rules);

    assert!(report.written.is_empty() && report.db_only.is_empty());
    assert_eq!(library.track(halo).unwrap().unwrap().title, "[HD]");
}

#[test]
fn stations_and_unknown_ids_are_skipped() {
    let mut station = track(1, "01 - Station (HD)", "", "", 0);
    station.kind = MediaKind::Radio;
    let mut library = Library::open_in_memory().unwrap();
    library
        .import_tracks(&[station, track(2, "02 - Song (HD)", "", "", 1)])
        .unwrap();

    let changes = library
        .clean_up_preview(&[1, 2, 2, 99], &all_rules())
        .unwrap();

    assert_eq!(changes.iter().map(|c| c.id).collect::<Vec<_>>(), [2]);
}

#[test]
fn a_cloud_placeholder_is_changed_in_the_library_only_and_never_opened() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    // One was evicted since the last scan, so the row still says it is local.
    let evicted = dir.path().join("01 - Evicted.mp3");
    let in_cloud = dir.path().join("02 - Cloud.mp3");
    let mut rows = Vec::new();
    for (id, path, in_cloud_flag) in [(1, &evicted, false), (2, &in_cloud, true)] {
        fs::write(path, b"placeholder").unwrap();
        fs::write(path.with_extension("evicted"), b"").unwrap();
        make_unreadable(path);
        let mut row = track(id, &format!("0{id} - Song {id}"), "", "", 1);
        row.path = path.to_str().unwrap().to_owned();
        row.in_cloud = in_cloud_flag;
        rows.push(row);
    }
    library.import_tracks(&rows).unwrap();

    let report = clean_up(&mut library, &[1, 2], &all_rules());

    make_readable(&evicted);
    make_readable(&in_cloud);
    assert!(report.written.is_empty());
    let reasons: Vec<_> = report
        .db_only
        .iter()
        .map(|(id, why)| (*id, why.to_lowercase()))
        .collect();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(
        reasons.iter().all(|(_, why)| why.contains("icloud")),
        "refused because of the cloud, not because a read failed: {reasons:?}"
    );
    for (id, name) in [(1, "Song 1"), (2, "Song 2")] {
        let row = library.track(id).unwrap().unwrap();
        assert_eq!(
            (row.title.as_str(), row.track_number),
            (name, Some(id as u32))
        );
    }
    assert_eq!(fs::read(&evicted).unwrap(), b"placeholder");
    assert_eq!(fs::read(&in_cloud).unwrap(), b"placeholder");
}

#[cfg(unix)]
fn make_unreadable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
}

#[cfg(unix)]
fn make_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

// Windows has no such permission, so there the dataless check alone keeps the file closed.
#[cfg(not(unix))]
fn make_unreadable(_path: &Path) {}

#[cfg(not(unix))]
fn make_readable(_path: &Path) {}

#[test]
fn progress_counts_the_songs_to_change_and_cancelling_stops_before_the_next() {
    let (mut library, _dir, ids) = untidy_library();
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();

    let report = library
        .clean_up(&ids, &all_rules(), &cancel, |done, total| {
            seen.push((done, total));
            // The first song is done, so the other two are not started.
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap();

    assert_eq!(seen, [(1, 3)]);
    assert_eq!(report.written.len(), 1);
    assert_eq!(
        library.clean_up_preview(&ids, &all_rules()).unwrap().len(),
        2,
        "what was not reached is still left to do"
    );
}

#[test]
fn the_preview_says_which_changes_fill_details_and_which_songs_are_in_the_cloud() {
    let mut cloud = track(1, "02 - Song", "Artist", "", 1);
    cloud.in_cloud = true;
    let mut library = Library::open_in_memory().unwrap();
    library
        .import_tracks(&[cloud, track(2, "Plain (HD)", "Artist", "", 1)])
        .unwrap();

    let changes = library.clean_up_preview(&[1, 2], &all_rules()).unwrap();

    let by_id = |id: i64| changes.iter().find(|c| c.id == id).unwrap();
    assert!(by_id(1).in_cloud && by_id(1).fills());
    assert!(!by_id(2).in_cloud && !by_id(2).fills());
}
