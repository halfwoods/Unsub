use antistreaming_ipod::{
    SyncPlan, SyncPlaylist, clear_play_counts, plan_sync, read_database,
    read_play_counts_for_sync_back,
};

use super::*;
use crate::common::db::{PlayCount, play_counts_file};

fn ipod_id_of(fake: &FakeIpod, title: &str) -> u32 {
    read_database(&opened(fake))
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.title == title)
        .unwrap()
        .id
}

fn path_of(fake: &FakeIpod, title: &str) -> PathBuf {
    read_database(&opened(fake))
        .unwrap()
        .tracks
        .iter()
        .find(|t| t.title == title)
        .unwrap()
        .path
        .clone()
}

#[test]
fn a_second_sync_adds_removes_and_updates_only_what_changed() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let a = library_track(library.path(), 1, "Alpha");
    let b = library_track(library.path(), 2, "Bravo");
    let c = library_track(library.path(), 3, "Charlie");
    sync(&fake, &[a.clone(), b.clone(), c.clone()], &[]);
    let alpha_path = path_of(&fake, "Alpha");
    let bravo_path = path_of(&fake, "Bravo");
    let bravo_id = ipod_id_of(&fake, "Bravo");
    let files_before = music_files(&fake);

    let mut renamed = c.clone();
    renamed.title = "Charlie (Remastered)".to_owned();
    renamed.rating = 100;
    let d = library_track(library.path(), 4, "Delta");
    let wanted = [a.clone(), renamed.clone(), d.clone()];

    let plan = plan(&fake, &wanted, &[]);

    assert_eq!(plan.add, [4]);
    assert_eq!(plan.update, [3]);
    assert_eq!(plan.remove, [bravo_id]);
    assert_eq!(plan.bytes_needed, 4000);

    apply_sync(
        &opened(&fake),
        &plan,
        &wanted,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    )
    .unwrap();

    let read = read_database(&opened(&fake)).unwrap();
    let titles: BTreeSet<_> = read.tracks.iter().map(|t| t.title.as_str()).collect();
    assert!(
        titles.contains("Alpha")
            && titles.contains("Delta")
            && titles.contains("Charlie (Remastered)")
    );
    assert!(!titles.contains("Bravo") && !titles.contains("Charlie"));
    assert_eq!(
        read.tracks.len(),
        5 + 3,
        "the five tracks iTunes put there are untouched"
    );
    let charlie = read
        .tracks
        .iter()
        .find(|t| t.title == "Charlie (Remastered)")
        .unwrap();
    assert_eq!(charlie.rating, 100);
    assert_eq!(
        path_of(&fake, "Alpha"),
        alpha_path,
        "an unchanged track is not copied again"
    );
    assert!(!bravo_path.exists(), "the removed track's file is deleted");
    let files_after = music_files(&fake);
    assert_eq!(
        files_after.len(),
        files_before.len(),
        "one file out, one in"
    );
    assert!(files_after.contains(&alpha_path));
    assert!(!files_after.contains(&bravo_path));
}

#[test]
fn syncing_again_with_nothing_changed_plans_nothing() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "Alpha"),
        library_track(library.path(), 2, "Bravo"),
    ];
    sync(&fake, &tracks, &[]);

    let plan = plan(&fake, &tracks, &[]);

    assert_eq!(
        plan,
        SyncPlan {
            add: vec![],
            update: vec![],
            remove: vec![],
            bytes_needed: 0,
            fits: true,
            removals_first: false,
        }
    );
}

#[test]
fn tracks_the_app_did_not_add_are_never_planned_for_removal() {
    let fake = fake_classic();
    let ipod = opened(&fake);
    let current = read_database(&ipod).unwrap();

    // The library is empty, yet the five tracks iTunes synced are left alone.
    let plan = plan_sync(&ipod, &current, &[], &[]).unwrap();

    assert!(plan.remove.is_empty());
}

#[test]
fn a_forged_plan_cannot_remove_a_track_the_app_did_not_add() {
    let fake = fake_classic();
    let ipod = opened(&fake);
    let before = db_bytes(&fake);
    let foreign = read_database(&ipod).unwrap().tracks[0].id;
    let forged = SyncPlan {
        add: vec![],
        update: vec![],
        remove: vec![foreign],
        bytes_needed: 0,
        fits: true,
        removals_first: false,
    };

    let result = apply_sync(
        &ipod,
        &forged,
        &[],
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert_eq!(db_bytes(&fake), before);
}

#[test]
fn removing_the_last_reference_deletes_the_file_but_a_shared_file_is_kept() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let a = library_track(library.path(), 1, "Alpha");
    sync(&fake, std::slice::from_ref(&a), &[]);
    let alpha = path_of(&fake, "Alpha");

    sync(&fake, &[], &[]);

    assert!(!alpha.exists());
    assert_eq!(read_database(&opened(&fake)).unwrap().tracks.len(), 5);
}

#[test]
fn managed_playlists_are_replaced_and_foreign_ones_survive() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "One"),
        library_track(library.path(), 2, "Two"),
    ];
    sync(
        &fake,
        &tracks,
        &[SyncPlaylist {
            name: "Mine".into(),
            tracks: vec![1, 2],
        }],
    );
    let first_id = read_database(&opened(&fake))
        .unwrap()
        .playlists
        .iter()
        .find(|p| p.name == "Mine")
        .unwrap()
        .id;

    sync(
        &fake,
        &tracks,
        &[
            SyncPlaylist {
                name: "Mine".into(),
                tracks: vec![2],
            },
            SyncPlaylist {
                name: "Extra".into(),
                tracks: vec![1],
            },
        ],
    );
    let read = read_database(&opened(&fake)).unwrap();
    let names: Vec<_> = read.playlists.iter().map(|p| p.name.as_str()).collect();
    let mine = read.playlists.iter().find(|p| p.name == "Mine").unwrap();
    assert_eq!(names.iter().filter(|n| **n == "Mine").count(), 1);
    assert!(names.contains(&"Extra") && names.contains(&"Road Trip"));
    assert_eq!(
        mine.id, first_id,
        "a playlist keeps its identity across syncs"
    );
    assert_eq!(mine.track_ids.len(), 1);

    sync(&fake, &tracks, &[]);
    let read = read_database(&opened(&fake)).unwrap();
    assert!(
        read.playlists
            .iter()
            .all(|p| p.name != "Mine" && p.name != "Extra")
    );
    assert!(read.playlists.iter().any(|p| p.name == "Road Trip"));
}

#[test]
fn play_counts_come_back_for_synced_tracks_only_and_are_cleared_on_request() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "One"),
        library_track(library.path(), 2, "Two"),
    ];
    sync(&fake, &tracks, &[]);
    // Entries follow track order: five foreign tracks, then One, then Two.
    let mut entries: Vec<PlayCount> = (0..5)
        .map(|_| PlayCount {
            plays: 9,
            last_played_mac: 0,
            rating: 0,
            skips: 0,
        })
        .collect();
    entries.push(PlayCount {
        plays: 0,
        last_played_mac: 0,
        rating: 0,
        skips: 0,
    });
    entries.push(PlayCount {
        plays: 3,
        last_played_mac: crate::mac_wall_clock(2012, 6, 1, 12),
        rating: 100,
        skips: 2,
    });
    fake.write_play_counts(&play_counts_file(&entries));
    let ipod = opened(&fake);

    let deltas = read_play_counts_for_sync_back(&ipod).unwrap();

    assert_eq!(deltas.len(), 1, "unplayed and foreign tracks are left out");
    let (library_id, delta) = &deltas[0];
    assert_eq!(*library_id, 2);
    assert_eq!((delta.plays, delta.skips), (3, 2));
    assert_eq!(delta.rating, Some(100));
    assert_eq!(delta.last_played, Some(unix_local(2012, 6, 1, 12)));
    assert!(
        itunes_file(&fake, "Play Counts").exists(),
        "reading does not consume the file"
    );

    // A sync does not touch the file either; clearing is the caller's decision.
    sync(&fake, &tracks, &[]);
    assert!(itunes_file(&fake, "Play Counts").exists());

    clear_play_counts(&ipod).unwrap();
    assert!(!itunes_file(&fake, "Play Counts").exists());
    clear_play_counts(&ipod).unwrap();
    assert!(read_play_counts_for_sync_back(&ipod).unwrap().is_empty());
}

fn sound_check_of(fake: &FakeIpod, title: &str) -> (u32, u32) {
    let read = read_database(&opened(fake)).unwrap();
    let track = read.tracks.iter().find(|t| t.title == title).unwrap();
    let (header, _) = crate::common::db::track_record(&db_bytes(fake), track.dbid).unwrap();
    (track.sound_check, crate::common::db::u32_at(&header, 76))
}

#[test]
fn sound_check_is_written_and_a_change_to_it_is_an_update() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut loud = library_track(library.path(), 1, "Loud");
    loud.sound_check = Some(1_259);
    let quiet = library_track(library.path(), 2, "Quiet");
    sync(&fake, &[loud.clone(), quiet.clone()], &[]);

    assert_eq!(sound_check_of(&fake, "Loud"), (1_259, 1_259));
    assert_eq!(sound_check_of(&fake, "Quiet"), (0, 0), "no value means off");
    assert!(
        plan(&fake, &[loud.clone(), quiet.clone()], &[])
            .update
            .is_empty()
    );

    loud.sound_check = Some(800);
    let (changed, _) = sync(&fake, &[loud.clone(), quiet.clone()], &[]);
    assert_eq!(changed.update, [1]);
    assert_eq!(sound_check_of(&fake, "Loud"), (800, 800));

    loud.sound_check = None;
    let (switched_off, _) = sync(&fake, &[loud, quiet], &[]);
    assert_eq!(switched_off.update, [1]);
    assert_eq!(sound_check_of(&fake, "Loud"), (0, 0));
}

#[test]
fn syncing_never_changes_the_sound_check_itunes_gave_its_own_tracks() {
    let mut database = sample_database();
    database.tracks[0]
        .header_patches
        .push((76, 1_995u32.to_le_bytes().to_vec()));
    let fake = crate::classic_7g(&database);
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "Alpha")], &[]);

    let read = read_database(&opened(&fake)).unwrap();
    let crazy = read
        .tracks
        .iter()
        .find(|t| t.title == "Crazy in Love")
        .unwrap();
    assert_eq!(crazy.sound_check, 1_995);
}
