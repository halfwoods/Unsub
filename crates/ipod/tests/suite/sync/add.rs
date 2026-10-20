use std::fs;

use antistreaming_ipod::{IpodMediaKind, SyncPlaylist, read_database};

use super::*;

#[test]
fn a_new_track_reads_back_with_every_field() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let track = library_track(library.path(), 7, "Hunter");

    let (plan, written) = sync(&fake, std::slice::from_ref(&track), &[]);

    assert_eq!(
        (plan.add.clone(), plan.update.len(), plan.remove.len()),
        (vec![7], 0, 0)
    );
    assert_eq!(plan.bytes_needed, 4000);
    assert!(plan.fits);
    let read = read_database(&opened(&fake)).unwrap();
    assert_eq!(read.tracks.len(), 6);
    let on_ipod = read.tracks.iter().find(|t| t.title == "Hunter").unwrap();
    assert_eq!(on_ipod.artist, "Björk");
    assert_eq!(on_ipod.album_artist, "Björk");
    assert_eq!(on_ipod.album, "Homogenic");
    assert_eq!(on_ipod.genre, "Electronic");
    assert_eq!(on_ipod.composer, "Guðmundur");
    assert_eq!(on_ipod.year, Some(1997));
    assert_eq!(on_ipod.track_number, Some(7));
    assert_eq!(on_ipod.track_count, Some(10));
    assert_eq!(on_ipod.disc_number, Some(1));
    assert_eq!(on_ipod.disc_count, Some(1));
    assert_eq!(on_ipod.duration_ms, 245_000);
    assert_eq!(on_ipod.bitrate_kbps, Some(256));
    assert_eq!(on_ipod.sample_rate, Some(44_100));
    assert_eq!(on_ipod.file_size, 4000);
    assert_eq!(on_ipod.play_count, 4);
    assert_eq!(on_ipod.last_played, Some(unix_local(2011, 3, 4, 12)));
    assert_eq!(on_ipod.date_added, Some(unix_local(2010, 1, 2, 12)));
    assert_eq!(on_ipod.rating, 60);
    assert_eq!(on_ipod.media_kind, IpodMediaKind::Music);
    assert_eq!(on_ipod.file_type, "MPEG audio file");
    assert!(!on_ipod.compilation);
    assert_eq!(on_ipod.sort_title.as_deref(), Some("sort Hunter"));
    assert_eq!(on_ipod.sort_artist.as_deref(), Some("Bjork"));
    assert_eq!(on_ipod.sort_album, None);
    assert_eq!(on_ipod.sort_album_artist.as_deref(), Some("Bjork"));
    assert_ne!(on_ipod.dbid, 0);
    // What apply_sync returns is what a fresh read finds, apart from the stale Play Counts merge.
    assert_eq!(written.tracks.len(), 6);
}

#[test]
fn the_music_file_lands_in_a_music_folder_under_a_four_letter_name() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let track = library_track(library.path(), 7, "Hunter");

    sync(&fake, std::slice::from_ref(&track), &[]);

    let files = music_files(&fake);
    assert_eq!(files.len(), 1);
    let path = files.iter().next().unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    let (stem, extension) = name.split_once('.').unwrap();
    assert_eq!(stem.len(), 4);
    assert!(stem.bytes().all(|b| b.is_ascii_uppercase()));
    assert_eq!(extension, "mp3");
    let folder = path
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    assert!(["F00", "F01", "F02"].contains(&folder), "{folder}");
    assert_eq!(fs::read(path).unwrap(), vec![7u8; 4000]);
    let read = read_database(&opened(&fake)).unwrap();
    assert_eq!(
        read.tracks
            .iter()
            .find(|t| t.title == "Hunter")
            .unwrap()
            .path,
        *path
    );
    assert!(
        library.path().join("7.mp3").exists(),
        "the source file is never touched"
    );
}

#[test]
fn tracks_already_on_the_ipod_are_carried_over_unchanged() {
    let fake = fake_classic();
    let before = read_database(&opened(&fake)).unwrap();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 7, "Hunter")], &[]);

    let after = read_database(&opened(&fake)).unwrap();
    assert_eq!(after.tracks[..5], before.tracks[..]);
    assert_eq!(after.playlists[1..], before.playlists[1..]);
    assert_eq!(
        after.playlists[0].track_ids.len(),
        6,
        "the master playlist gains the new track"
    );
}

#[test]
fn fields_this_crate_does_not_model_survive_byte_for_byte() {
    let mut gapless = Track::new(1, "Gapless Song");
    gapless.dbid = 0x1111_2222_3333_4444;
    gapless.header_patches = vec![
        (64, vec![0xfb, 0xff, 0xff, 0xff]),           // volume adjustment
        (76, 1234u32.to_le_bytes().to_vec()),         // sound check
        (184, 2112u32.to_le_bytes().to_vec()),        // pregap
        (188, 9_999_999u64.to_le_bytes().to_vec()),   // sample count
        (248, 0x00ab_cdefu32.to_le_bytes().to_vec()), // gapless data
        (352, 77u32.to_le_bytes().to_vec()),          // artwork link
    ];
    gapless.extra_mhods.push(db::mhod_text(
        8,
        "a comment iTunes wrote",
        db::Encoding::Utf16,
    ));
    let database = DbSpec::new(vec![gapless], vec![Playlist::master(1, "Pod", &[1])]);
    let fake = FakeIpod::new("POD");
    fake.write_itunesdb(&database.build());
    fs::create_dir_all(fake.mount.join("iPod_Control/Music/F00")).unwrap();
    fake.write_sysinfo("xB029", "0x000A27001A2B3C4D");
    let before = db::track_record(&db_bytes(&fake), 0x1111_2222_3333_4444).unwrap();
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 9, "New")], &[]);

    let after = db::track_record(&db_bytes(&fake), 0x1111_2222_3333_4444).unwrap();
    // The album id (+0x120) is renumbered with the album list; everything else is identical.
    let mut expected_header = before.0.clone();
    expected_header[0x120..0x124].copy_from_slice(&after.0[0x120..0x124]);
    assert_eq!(after.0, expected_header);
    assert_eq!(
        after.1, before.1,
        "mhods are copied verbatim, including ones the crate ignores"
    );
}

#[test]
fn an_old_style_database_keeps_its_short_headers_and_missing_sections() {
    let mut spec = DbSpec::new(sample_tracks(), sample_playlists());
    spec.mhit_header = 0x9c;
    spec.podcast_section = false;
    let fake = FakeIpod::new("OLDPOD");
    fake.write_itunesdb(&spec.build());
    fs::create_dir_all(fake.mount.join("iPod_Control/Music/F00")).unwrap();
    fake.write_sysinfo("x8541", "0x000A27001A2B3C4D");
    let library = tempfile::tempdir().unwrap();

    sync(&fake, &[library_track(library.path(), 7, "Hunter")], &[]);

    let bytes = db_bytes(&fake);
    assert!(
        db::chunks(&bytes, b"mhit", 0x9c).len() == 6,
        "all six tracks use the old header size"
    );
    let read = read_database(&opened(&fake)).unwrap();
    let new = read.tracks.iter().find(|t| t.title == "Hunter").unwrap();
    assert_eq!(new.artist, "Björk");
    assert_eq!(
        new.skip_count, 0,
        "fields past the short header are simply absent"
    );
}

#[test]
fn m4a_files_are_labelled_by_codec() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut lossless = library_track(library.path(), 1, "Lossless");
    lossless.source_path = library.path().join("lossless.m4a");
    fs::write(&lossless.source_path, mp4_with_sample_entry(b"alac")).unwrap();
    let mut aac = library_track(library.path(), 2, "Aac");
    aac.source_path = library.path().join("aac.m4a");
    fs::write(&aac.source_path, mp4_with_sample_entry(b"mp4a")).unwrap();

    sync(&fake, &[lossless, aac], &[]);

    let read = read_database(&opened(&fake)).unwrap();
    let type_of = |title: &str| {
        read.tracks
            .iter()
            .find(|t| t.title == title)
            .unwrap()
            .file_type
            .clone()
    };
    assert_eq!(type_of("Lossless"), "Apple Lossless audio file");
    assert_eq!(type_of("Aac"), "AAC audio file");
}

fn mp4_with_sample_entry(entry: &[u8; 4]) -> Vec<u8> {
    fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut atom = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        atom.extend_from_slice(kind);
        atom.extend_from_slice(body);
        atom
    }
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
        atom(b"mdat", &[0; 64]),
        moov,
    ]
    .concat()
}

#[test]
fn the_prepared_file_is_what_gets_copied_and_the_source_survives() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let flac_like = library_track(library.path(), 5, "Transcoded");
    let transcoded = library.path().join("transcoded.m4a");
    fs::write(&transcoded, mp4_with_sample_entry(b"mp4a")).unwrap();
    let plan = plan(&fake, std::slice::from_ref(&flac_like), &[]);

    apply_sync(
        &opened(&fake),
        &plan,
        std::slice::from_ref(&flac_like),
        &[],
        &mut |_| Ok(transcoded.clone()),
        &mut no_cover,
        &mut |_| {},
    )
    .unwrap();

    let read = read_database(&opened(&fake)).unwrap();
    let on_ipod = read
        .tracks
        .iter()
        .find(|t| t.title == "Transcoded")
        .unwrap();
    assert_eq!(on_ipod.path.extension().unwrap(), "m4a");
    assert_eq!(on_ipod.file_size, fs::metadata(&transcoded).unwrap().len());
    assert!(transcoded.exists() && flac_like.source_path.exists());
}

#[test]
fn podcasts_go_to_the_podcasts_playlist_not_the_master_playlist() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let mut episode = library_track(library.path(), 3, "Episode 9");
    episode.media_kind = IpodMediaKind::Podcast;

    sync(&fake, &[episode], &[]);

    let read = read_database(&opened(&fake)).unwrap();
    let id = read
        .tracks
        .iter()
        .find(|t| t.title == "Episode 9")
        .unwrap()
        .id;
    let podcasts = read.playlists.iter().find(|p| p.is_podcasts).unwrap();
    let master = read.playlists.iter().find(|p| p.is_master).unwrap();
    assert!(podcasts.track_ids.contains(&id));
    assert!(!master.track_ids.contains(&id));
    assert_eq!(
        read.tracks.iter().find(|t| t.id == id).unwrap().media_kind,
        IpodMediaKind::Podcast
    );
}

#[test]
fn playlists_are_written_and_keep_their_order() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 1, "One"),
        library_track(library.path(), 2, "Two"),
        library_track(library.path(), 3, "Three"),
    ];
    let playlists = [SyncPlaylist {
        name: "Mine".to_owned(),
        tracks: vec![3, 1, 99],
    }];

    sync(&fake, &tracks, &playlists);

    let read = read_database(&opened(&fake)).unwrap();
    let mine = read.playlists.iter().find(|p| p.name == "Mine").unwrap();
    let title_of = |id: u32| {
        read.tracks
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .title
            .clone()
    };
    let titles: Vec<_> = mine.track_ids.iter().map(|&id| title_of(id)).collect();
    assert_eq!(titles, ["Three", "One"], "unknown library ids are skipped");
    assert!(!mine.is_master && !mine.is_podcasts && !mine.is_smart);
    assert_ne!(mine.id, 0);
    let foreign: Vec<_> = read
        .playlists
        .iter()
        .map(|p| p.name.as_str())
        .filter(|n| *n != "Mine")
        .collect();
    assert_eq!(
        foreign,
        ["Riley’s iPod", "Road Trip", "Top Rated", "Podcasts"]
    );
}

#[test]
fn the_bookkeeping_file_records_what_was_added() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let tracks = [
        library_track(library.path(), 7, "Hunter"),
        library_track(library.path(), 8, "Bachelorette"),
    ];
    let playlists = [SyncPlaylist {
        name: "Mine".to_owned(),
        tracks: vec![7],
    }];

    sync(&fake, &tracks, &playlists);

    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(itunes_file(&fake, "AntiStreaming.json")).unwrap())
            .unwrap();
    assert_eq!(json["version"], 1);
    let mapped: Vec<_> = json["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["libraryId"].as_i64().unwrap())
        .collect();
    assert_eq!(mapped, [7, 8]);
    let read = read_database(&opened(&fake)).unwrap();
    for entry in json["tracks"].as_array().unwrap() {
        let dbid = entry["dbid"].as_u64().unwrap();
        assert!(read.tracks.iter().any(|t| t.dbid == dbid));
    }
    assert_eq!(json["playlists"][0]["name"], "Mine");
}

#[test]
fn the_returned_database_matches_what_a_fresh_read_finds() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();

    let (_, written) = sync(&fake, &[library_track(library.path(), 7, "Hunter")], &[]);

    assert_eq!(written, read_database(&opened(&fake)).unwrap());
}
