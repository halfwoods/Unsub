//! Reading an iPod to copy its music out. None of it may change the iPod.

use std::collections::BTreeMap;

use unsub_ipod::{
    Error, IpodTrack, ensure_room, free_space, holds_protected_audio, is_copy_protected,
    read_database, read_rescue,
};

use super::*;
use crate::common::db::on_the_go_file;

fn music_file(fake: &FakeIpod, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = fake.mount.join("iPod_Control/Music").join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut atom = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    atom.extend_from_slice(kind);
    atom.extend_from_slice(body);
    atom
}

/// An MPEG-4 audio file whose only sample description is `entry`.
fn mp4_with_entry(entry: &[u8; 4]) -> Vec<u8> {
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend_from_slice(&36u32.to_be_bytes());
    stsd.extend_from_slice(entry);
    let stbl = atom(b"stbl", &atom(b"stsd", &stsd));
    let moov = atom(
        b"moov",
        &atom(b"trak", &atom(b"mdia", &atom(b"minf", &stbl))),
    );
    [
        atom(b"ftyp", b"M4A \0\0\0\0"),
        atom(b"mdat", &[1, 2, 3]),
        moov,
    ]
    .concat()
}

fn track_at(path: &Path, file_type: &str) -> IpodTrack {
    IpodTrack {
        path: path.to_path_buf(),
        file_type: file_type.to_owned(),
        ..IpodTrack::default()
    }
}

/// Every file on the iPod with its bytes, to prove a read left it as it was.
fn contents(fake: &FakeIpod) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(&fake.mount, &mut found);
    found
}

/// The sample library as iTunes left it, three songs this app added after it, and their playlist.
fn fake_with_both() -> (FakeIpod, tempfile::TempDir) {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "Alpha"),
        library_track(library.path(), 2, "Bravo"),
        library_track(library.path(), 3, "Charlie"),
    ];
    sync(
        &fake,
        &tracks,
        &[SyncPlaylist {
            name: "Mine".into(),
            tracks: vec![1, 2],
        }],
    );
    (fake, library)
}

#[test]
fn songs_say_which_the_app_synced_and_how_big_their_files_are() {
    let (fake, _library) = fake_with_both();
    music_file(&fake, "F00/TRK001.mp3", &[7; 123]);

    let rescue = read_rescue(&opened(&fake)).unwrap();

    assert!(!rescue.from_files);
    let by_title = |title: &str| {
        rescue
            .songs
            .iter()
            .find(|song| song.track.title == title)
            .unwrap()
    };
    assert_eq!(by_title("Crazy in Love").library_id, None);
    assert_eq!(by_title("Crazy in Love").bytes, Some(123));
    assert_eq!(by_title("Crazy in Love").track.artist, "Beyoncé");
    // The database says 7,000,000 bytes, but there is no file to copy.
    assert_eq!(
        (
            by_title("戦場のメリークリスマス").library_id,
            by_title("戦場のメリークリスマス").bytes
        ),
        (None, None)
    );
    let synced: Vec<_> = ["Alpha", "Bravo", "Charlie"]
        .map(by_title)
        .into_iter()
        .collect();
    assert_eq!(
        synced
            .iter()
            .map(|song| song.library_id)
            .collect::<Vec<_>>(),
        [Some(1), Some(2), Some(3)]
    );
    assert!(synced.iter().all(|song| song.bytes == Some(4000)));
    assert_eq!(rescue.songs.len(), 8);
}

#[test]
fn playlists_leave_out_the_master_the_podcasts_and_the_ones_the_app_made() {
    let (fake, _library) = fake_with_both();

    let rescue = read_rescue(&opened(&fake)).unwrap();

    let names: Vec<_> = rescue
        .playlists
        .iter()
        .map(|playlist| playlist.name.as_str())
        .collect();
    assert_eq!(names, ["Road Trip", "Top Rated"]);
    let road_trip = &rescue.playlists[0];
    assert_eq!(
        (road_trip.key.as_str(), road_trip.smart, road_trip.on_the_go),
        ("4097", false, false)
    );
    // The file lists its members out of order; their positions put them right.
    assert_eq!(road_trip.track_ids, [1, 5, 2]);
    assert!(rescue.playlists[1].smart);
}

#[test]
fn on_the_go_playlists_come_last_and_keep_tracks_that_itunes_added() {
    let (fake, _library) = fake_with_both();
    let ipod = opened(&fake);
    let database = read_database(&ipod).unwrap();
    // Positions in the track list: 0 is iTunes' own, 5 is the app's.
    fs::write(
        itunes_file(&fake, "OTGPlaylistInfo"),
        on_the_go_file(&[5, 0], 0x14, 4, false),
    )
    .unwrap();
    fs::write(
        itunes_file(&fake, "OTGPlaylistInfo_1"),
        on_the_go_file(&[1], 0x14, 4, false),
    )
    .unwrap();

    let rescue = read_rescue(&ipod).unwrap();

    let otg: Vec<_> = rescue
        .playlists
        .iter()
        .filter(|playlist| playlist.on_the_go)
        .collect();
    assert_eq!(
        otg.iter()
            .map(|playlist| playlist.name.as_str())
            .collect::<Vec<_>>(),
        ["On-The-Go 1", "On-The-Go 2"]
    );
    assert_eq!(
        otg.iter()
            .map(|playlist| playlist.key.as_str())
            .collect::<Vec<_>>(),
        ["otg1", "otg2"]
    );
    assert_eq!(
        otg[0].track_ids,
        [database.tracks[5].id, database.tracks[0].id]
    );
    assert_eq!(otg[1].track_ids, [database.tracks[1].id]);
    assert!(rescue.playlists.last().unwrap().on_the_go);
}

#[test]
fn a_damaged_on_the_go_file_costs_only_those_playlists() {
    let (fake, _library) = fake_with_both();
    fs::write(itunes_file(&fake, "OTGPlaylistInfo"), b"not a playlist").unwrap();

    let rescue = read_rescue(&opened(&fake)).unwrap();

    assert_eq!(rescue.songs.len(), 8);
    assert!(rescue.playlists.iter().all(|playlist| !playlist.on_the_go));
}

#[test]
fn an_ipod_without_a_database_lists_the_music_files_for_their_tags_to_describe() {
    let fake = FakeIpod::new("SHUFFLE");
    fake.write_sysinfo("xC293", "0x000A27001A2B3C4D");
    music_file(&fake, "F01/BBBB.mp3", &[1; 10]);
    music_file(&fake, "F01/BOOK.aax", &[6; 7]);
    music_file(&fake, "F00/AAAA.m4a", &[2; 20]);
    music_file(&fake, "F00/.hidden.mp3", &[3; 5]);
    music_file(&fake, "F00/notes.txt", &[4; 5]);

    let rescue = read_rescue(&opened(&fake)).unwrap();

    assert!(rescue.from_files && rescue.playlists.is_empty());
    let found: Vec<_> = rescue
        .songs
        .iter()
        .map(|song| {
            (
                song.track.path.file_name().unwrap().to_str().unwrap(),
                song.bytes,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("AAAA.m4a", Some(20)),
            ("BBBB.mp3", Some(10)),
            ("BOOK.aax", Some(7))
        ]
    );
    assert!(
        rescue
            .songs
            .iter()
            .all(|song| song.track.title.is_empty() && song.library_id.is_none())
    );
    assert_eq!(
        rescue
            .songs
            .iter()
            .map(|song| song.track.id)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn an_ipod_with_no_music_folders_and_no_database_has_nothing_to_rescue() {
    let fake = FakeIpod::new("EMPTY");
    fake.write_sysinfo("xC293", "0x000A27001A2B3C4D");
    fs::remove_dir_all(fake.mount.join("iPod_Control/Music")).unwrap();

    let rescue = read_rescue(&opened(&fake)).unwrap();

    assert!(rescue.songs.is_empty());
}

#[test]
fn protected_songs_are_known_by_name_and_type_without_opening_them() {
    let fake = FakeIpod::new("DRM");
    let plain = music_file(&fake, "F00/PLAN.m4a", &mp4_with_entry(b"mp4a"));
    let mp3 = music_file(&fake, "F00/SONG.mp3", b"ID3 not mpeg-4");

    // By extension alone, even when the file is not there to open.
    for name in ["GONE.m4p", "BOOK.aa", "BOOK.aax", "BOOK.AAX"] {
        assert!(
            is_copy_protected(&track_at(&plain.with_file_name(name), "")),
            "{name}"
        );
    }
    // By the type string iTunes stored, which a flash iPod replaces with the extension.
    assert!(is_copy_protected(&track_at(
        &plain,
        "Protected AAC audio file"
    )));
    assert!(is_copy_protected(&track_at(&plain, "Audible file")));
    assert!(is_copy_protected(&track_at(&mp3, "M4P ")));
    // A file that gives no sign by name is not found out by this: that takes opening it.
    let disguised = music_file(&fake, "F00/RENM.m4a", &mp4_with_entry(b"drms"));
    assert!(!is_copy_protected(&track_at(&disguised, "AAC audio file")));
    assert!(!is_copy_protected(&track_at(&plain, "AAC audio file")));
    assert!(!is_copy_protected(&track_at(&mp3, "MPEG audio file")));
}

#[test]
fn a_protected_file_with_an_innocent_name_is_found_by_what_it_holds() {
    let fake = FakeIpod::new("DRM");
    let fairplay = music_file(&fake, "F00/RENM.m4a", &mp4_with_entry(b"drms"));
    let generic = music_file(&fake, "F00/ENCR.m4b", &mp4_with_entry(b"enca"));
    let audible = music_file(&fake, "F00/BOOK.m4b", &mp4_with_entry(b"aavd"));
    let plain = music_file(&fake, "F00/PLAN.m4a", &mp4_with_entry(b"mp4a"));
    let lossless = music_file(&fake, "F00/LOSS.m4a", &mp4_with_entry(b"alac"));
    let mp3 = music_file(&fake, "F00/SONG.mp3", b"ID3 not mpeg-4");

    for protected in [&fairplay, &generic, &audible] {
        assert!(
            holds_protected_audio(&track_at(protected, "AAC audio file")),
            "{protected:?}"
        );
    }
    for fine in [&plain, &lossless, &mp3, &fake.mount.join("MISSING.m4a")] {
        assert!(
            !holds_protected_audio(&track_at(fine, "AAC audio file")),
            "{fine:?}"
        );
    }
}

#[test]
fn reading_for_a_rescue_changes_nothing_on_the_ipod() {
    let (fake, _library) = fake_with_both();
    music_file(&fake, "F00/TRK001.mp3", &[7; 123]);
    music_file(&fake, "F00/SOLD.m4p", &mp4_with_entry(b"drms"));
    fs::write(
        itunes_file(&fake, "OTGPlaylistInfo"),
        on_the_go_file(&[5, 0], 0x14, 4, false),
    )
    .unwrap();
    let before = contents(&fake);
    let modified = fs::metadata(fake.mount.join(DB_FILE))
        .unwrap()
        .modified()
        .unwrap();

    let ipod = opened(&fake);
    let rescue = read_rescue(&ipod).unwrap();
    let protected = rescue
        .songs
        .iter()
        .filter(|song| is_copy_protected(&song.track))
        .count();
    assert_eq!(protected, 0);
    ensure_room(1, &fake.mount).unwrap();

    assert_eq!(contents(&fake), before);
    assert_eq!(
        fs::metadata(fake.mount.join(DB_FILE))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}

#[test]
fn a_copy_that_does_not_fit_is_refused_naming_the_folder() {
    let folder = tempfile::tempdir().unwrap();

    // Other things write to the disk meanwhile, so the free space is only known to be some.
    assert!(free_space(folder.path()).unwrap() > 0);
    ensure_room(0, folder.path()).unwrap();
    let error = ensure_room(u64::MAX, folder.path()).unwrap_err();

    assert!(
        matches!(&error, Error::NoRoom { folder: named, needed: u64::MAX, .. } if named == folder.path()),
        "{error}"
    );
}
