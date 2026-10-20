use std::fs;

use unsub_ipod::{SyncPlaylist, read_database};

use super::*;
use crate::common::db::{Database as DbSpec, Playlist};
use crate::common::shuffle::{Sd, parse_fixed, parse_indexed, playlist_counts};
use crate::{sample_database, sample_playlists, sample_tracks};

fn fake_shuffle(model: &str, database: &DbSpec) -> FakeIpod {
    let fake = FakeIpod::new("SHUFFLE");
    fake.write_itunesdb(&database.build());
    fake.write_sysinfo(model, "0x000A270011223344");
    fake
}

fn itunessd(fake: &FakeIpod) -> Vec<u8> {
    fs::read(itunes_file(fake, "iTunesSD")).unwrap()
}

/// `/iPod_Control/Music/F00/ABCD.mp3`, how a shuffle names a file.
fn shuffle_path(fake: &FakeIpod, native: &Path) -> String {
    let relative = native.strip_prefix(&fake.mount).unwrap();
    let parts: Vec<_> = relative
        .components()
        .map(|part| part.as_os_str().to_str().unwrap())
        .collect();
    format!("/{}", parts.join("/"))
}

fn assert_lists_the_tracks_of_the_itunesdb(fake: &FakeIpod, sd: &Sd) {
    let read = read_database(&opened(fake)).unwrap();
    let listed: Vec<_> = sd.tracks.iter().map(|track| track.path.clone()).collect();
    let expected: Vec<_> = read
        .tracks
        .iter()
        .map(|track| shuffle_path(fake, &track.path))
        .collect();
    assert_eq!(listed, expected);
}

#[test]
fn a_second_generation_shuffle_gets_fixed_records_in_the_itunesdb_track_order() {
    let mut database = sample_database();
    database.tracks[0].header_patches.push((165, vec![1]));
    database.tracks[1]
        .header_patches
        .push((68, 60_000u32.to_le_bytes().to_vec()));
    database.tracks[1]
        .header_patches
        .push((72, 120_000u32.to_le_bytes().to_vec()));
    database.tracks[3]
        .header_patches
        .push((64, (-255i32).to_le_bytes().to_vec()));
    database.tracks[4]
        .header_patches
        .push((64, 255i32.to_le_bytes().to_vec()));
    database.tracks[4].header_patches.push((166, vec![1]));
    let fake = fake_shuffle("xB518", &database);
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "Alpha"),
        library_track(library.path(), 2, "Bravo"),
    ];

    sync(&fake, &tracks, &[]);

    let sd = parse_fixed(&itunessd(&fake));
    assert_eq!(sd.tracks.len(), 7);
    assert_lists_the_tracks_of_the_itunesdb(&fake, &sd);
    let file_types: Vec<_> = sd.tracks.iter().map(|track| track.file_type).collect();
    assert_eq!(file_types[..2], [1, 2], "an mp3 and an AAC file");
    let plays: Vec<_> = sd
        .tracks
        .iter()
        .map(|track| track.plays_when_shuffling)
        .collect();
    assert_eq!(
        plays,
        [false, true, true, true, true, true, true],
        "only the track iTunes marked to skip when shuffling is skipped"
    );
    assert_eq!(
        (sd.tracks[1].start, sd.tracks[1].stop),
        (234 * 256, 468 * 256),
        "start and stop are kept in steps of 256 ms"
    );
    let volumes: Vec<_> = sd.tracks.iter().map(|track| track.volume).collect();
    assert_eq!(volumes[3..5], [0, 200], "-255..255 becomes 0..200");
    assert_eq!(volumes[0], 100, "no adjustment is the middle");
    assert!(sd.tracks[4].bookmarkable && !sd.tracks[0].bookmarkable);
}

#[test]
fn a_third_generation_shuffle_gets_chunks_with_voiceover_off_and_playlists() {
    let mut playlists = sample_playlists();
    playlists.push(Playlist::new(0x1004, "Books", &[4]));
    playlists.push(Playlist::new(0x1005, "Empty", &[]));
    let fake = fake_shuffle("xC306", &DbSpec::new(sample_tracks(), playlists));
    // What iTunes left: a volume limit, and VoiceOver on.
    let mut left_by_itunes = vec![0u8; 64];
    left_by_itunes[..4].copy_from_slice(b"bdhs");
    left_by_itunes[28] = 77;
    left_by_itunes[29] = 1;
    fs::write(itunes_file(&fake, "iTunesSD"), &left_by_itunes).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "Alpha"),
        library_track(library.path(), 2, "Bravo"),
    ];
    let mix = SyncPlaylist {
        name: "Mix".to_owned(),
        tracks: vec![2, 1],
    };

    sync(&fake, &tracks, std::slice::from_ref(&mix));

    let bytes = itunessd(&fake);
    let sd = parse_indexed(&bytes);
    let read = read_database(&opened(&fake)).unwrap();
    assert_eq!(sd.tracks.len(), 7);
    assert_lists_the_tracks_of_the_itunesdb(&fake, &sd);
    assert_eq!(sd.volume_limit, 77, "the limit set in iTunes is kept");
    assert!(!sd.voiceover);
    assert_eq!(
        sd.non_spoken_tracks, 5,
        "the podcast and the audiobook are apart"
    );
    let dbids: Vec<_> = read.tracks.iter().map(|track| track.dbid).collect();
    assert_eq!(sd.tracks.iter().map(|t| t.dbid).collect::<Vec<_>>(), dbids);
    let alpha = &sd.tracks[5];
    let bravo = &sd.tracks[6];
    assert_eq!(alpha.album_id, bravo.album_id, "one album, one id");
    assert_eq!(alpha.artist_id, bravo.artist_id);
    assert_ne!(alpha.album_id, sd.tracks[0].album_id);
    assert_eq!((alpha.track_number, alpha.disc_number), (1, 1));
    assert_eq!((alpha.start, alpha.stop, alpha.volume), (0, 0, 0));
    assert!(alpha.plays_when_shuffling);

    let by_id = |id: u64| sd.playlists.iter().find(|p| p.id == id).unwrap();
    let master = sd.playlists.iter().find(|p| p.kind == 1).unwrap();
    assert_eq!(master.members, [0, 1, 2, 3, 4, 5, 6]);
    assert_eq!((master.id, master.non_spoken_count), (0, 5));
    assert_eq!(by_id(0x1001).members, [0, 4, 1], "in playlist order");
    assert_eq!(by_id(0x1001).kind, 2);
    assert_eq!(by_id(0x1002).kind, 2, "a smart playlist is an ordinary one");
    let podcasts = by_id(0x1003);
    assert_eq!((podcasts.kind, &podcasts.members[..]), (3, &[2][..]));
    assert_eq!(podcasts.non_spoken_count, 0);
    let books = by_id(0x1004);
    assert_eq!((books.kind, &books.members[..]), (4, &[3][..]));
    let mix_id = read.playlists.iter().find(|p| p.name == "Mix").unwrap().id;
    let mix_on_sd = by_id(mix_id);
    assert_eq!((mix_on_sd.kind, &mix_on_sd.members[..]), (2, &[6, 5][..]));
    assert_eq!(sd.playlists.len(), 6, "the empty playlist is left out");
    assert_eq!(playlist_counts(&bytes), [6, 5, 1, 5]);
}

#[test]
fn the_sysinfo_extended_shadow_version_decides_the_layout_over_the_model() {
    let third = fake_shuffle("xC306", &sample_database());
    third.write_shadow_db_version(1);
    let second = fake_shuffle("xB518", &sample_database());
    second.write_shadow_db_version(2);
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "Alpha")];

    sync(&third, &tracks, &[]);
    sync(&second, &tracks, &[]);

    assert_eq!(parse_fixed(&itunessd(&third)).tracks.len(), 6);
    assert_eq!(parse_indexed(&itunessd(&second)).tracks.len(), 6);
}

#[test]
fn the_previous_itunessd_is_kept_aside_and_nothing_is_left_beside_it() {
    let fake = fake_shuffle("xB518", &sample_database());
    let from_itunes = b"the iTunesSD iTunes wrote".to_vec();
    fs::write(itunes_file(&fake, "iTunesSD"), &from_itunes).unwrap();
    let library = tempfile::tempdir().unwrap();
    let one = library_track(library.path(), 1, "Alpha");
    let two = library_track(library.path(), 2, "Bravo");

    sync(&fake, std::slice::from_ref(&one), &[]);
    let first = itunessd(&fake);
    sync(&fake, &[one, two], &[]);

    assert_ne!(first, from_itunes);
    assert_ne!(itunessd(&fake), first);
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesSD.unsub-backup")).unwrap(),
        first,
        "the backup is the file as it was before the latest sync"
    );
    assert_eq!(
        fs::read(itunes_file(&fake, "iTunesSD.unsub-original")).unwrap(),
        from_itunes,
        "the original is from before the first sync and is never overwritten"
    );
    assert!(
        itunes_dir_listing(&fake)
            .iter()
            .all(|name| !name.ends_with(".unsub-tmp"))
    );
}

#[test]
fn a_removed_track_leaves_the_itunessd_too() {
    let fake = fake_shuffle("xB518", &sample_database());
    let library = tempfile::tempdir().unwrap();
    let one = library_track(library.path(), 1, "Alpha");
    let two = library_track(library.path(), 2, "Bravo");
    sync(&fake, &[one.clone(), two], &[]);
    let removed = shuffle_path(&fake, &track_path(&fake, "Bravo"));
    assert!(
        parse_fixed(&itunessd(&fake))
            .tracks
            .iter()
            .any(|t| t.path == removed)
    );

    sync(&fake, &[one], &[]);

    let sd = parse_fixed(&itunessd(&fake));
    assert_eq!(sd.tracks.len(), 6);
    assert!(sd.tracks.iter().all(|track| track.path != removed));
    assert_lists_the_tracks_of_the_itunesdb(&fake, &sd);
}

#[test]
fn a_failure_staging_the_itunessd_stops_before_anything_is_replaced() {
    let fake = fake_shuffle("xB518", &sample_database());
    fs::create_dir(itunes_file(&fake, "iTunesSD.unsub-tmp")).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "Alpha")];
    let database = db_bytes(&fake);
    let music = music_files(&fake);
    let listing = itunes_dir_listing(&fake);
    let plan = plan(&fake, &tracks, &[]);

    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    assert!(result.is_err());
    assert_eq!(db_bytes(&fake), database, "the old iTunesDB is still there");
    assert_eq!(music_files(&fake), music, "copied music is removed again");
    assert_eq!(itunes_dir_listing(&fake), listing);
}

#[test]
fn a_failure_keeping_the_itunessd_backup_stops_before_anything_is_replaced() {
    let fake = fake_shuffle("xB518", &sample_database());
    fs::write(itunes_file(&fake, "iTunesSD"), b"from iTunes").unwrap();
    fs::create_dir(itunes_file(&fake, "iTunesSD.unsub-backup")).unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [library_track(library.path(), 1, "Alpha")];
    let database = db_bytes(&fake);
    let music = music_files(&fake);
    let plan = plan(&fake, &tracks, &[]);

    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut copy_as_is,
        &mut no_cover,
        &mut |_| {},
    );

    assert!(result.is_err());
    assert_eq!(db_bytes(&fake), database);
    assert_eq!(music_files(&fake), music);
    assert_eq!(itunessd(&fake), b"from iTunes");
}

#[test]
fn an_itunessd_changed_by_someone_else_mid_sync_is_not_overwritten() {
    let fake = fake_shuffle("xB518", &sample_database());
    fs::write(itunes_file(&fake, "iTunesSD"), b"from iTunes").unwrap();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "Alpha"),
        library_track(library.path(), 2, "Bravo"),
    ];
    let database = db_bytes(&fake);
    let plan = plan(&fake, &tracks, &[]);
    let path = itunes_file(&fake, "iTunesSD");

    let result = apply_sync(
        &opened(&fake),
        &plan,
        &tracks,
        &[],
        &mut |track| {
            if track.library_id == 2 {
                fs::write(&path, b"iTunes wrote this meanwhile").unwrap();
            }
            Ok(track.source_path.clone().into())
        },
        &mut no_cover,
        &mut |_| {},
    );

    assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
    assert_eq!(itunessd(&fake), b"iTunes wrote this meanwhile");
    assert_eq!(db_bytes(&fake), database);
}

#[test]
fn only_shuffles_get_an_itunessd() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 1, "Alpha")], &[]);

    assert!(!itunes_dir_listing(&fake).contains("iTunesSD"));
}
