use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};
use unsub_ipod::video::VideoKind;
use unsub_ipod::{
    VideoSyncTrack, apply_video_additions, plan_video_additions, restore_backup,
    video_add_supported,
};

use super::*;

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut bytes = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    bytes.extend(kind);
    bytes.extend(body);
    bytes
}

// Only container/sample-description correctness is tested here; the playback crate owns
// actual codec/profile inspection. These are never used as playable/hardware fixtures.
fn video_bytes(entry: &[u8; 4], seed: u8) -> Vec<u8> {
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend(atom(entry, &[0; 28]));
    let stbl = atom(b"stbl", &atom(b"stsd", &stsd));
    let moov = atom(
        b"moov",
        &atom(b"trak", &atom(b"mdia", &atom(b"minf", &stbl))),
    );
    [
        atom(b"ftyp", b"mp42\0\0\0\0"),
        moov,
        atom(b"mdat", &[seed; 100]),
    ]
    .concat()
}

fn video(library: &Path, id: i64, kind: VideoKind) -> VideoSyncTrack {
    let mut track = library_track(library, id, &format!("Video {id}"));
    track.source_path = library.join(format!("{id}.m4v"));
    track.media_kind = IpodMediaKind::Video;
    let bytes = video_bytes(b"avc1", id as u8);
    fs::write(&track.source_path, &bytes).unwrap();
    VideoSyncTrack {
        track,
        kind,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

fn add(fake: &FakeIpod, videos: &[VideoSyncTrack]) -> unsub_ipod::VideoAddResult {
    apply_video_additions(&opened(fake), videos, &AtomicBool::new(false), &mut |_| {}).unwrap()
}

fn mapping(fake: &FakeIpod) -> serde_json::Value {
    serde_json::from_slice(&fs::read(itunes_file(fake, "Unsub.json")).unwrap()).unwrap()
}

fn artwork_files(fake: &FakeIpod) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(artwork::art_dir(fake))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect()
}
use std::collections::BTreeMap;

#[test]
fn video_add_is_additive_and_a_later_music_sync_never_owns_or_removes_videos() {
    let (fake, _) = artwork::classic_with_foreign_artwork();
    let library = tempfile::tempdir().unwrap();
    let audio = library_track(library.path(), 1, "Owned audio");
    let mix = SyncPlaylist {
        name: "Owned mix".into(),
        tracks: vec![1, 1],
    };
    sync(&fake, &[audio], &[mix]);
    let before = read_database(&opened(&fake)).unwrap();
    let before_mapping = mapping(&fake);
    let before_art = artwork_files(&fake);
    let videos = [
        video(library.path(), 1, VideoKind::Movie),
        video(library.path(), 2, VideoKind::MusicVideo),
    ];
    let result = add(&fake, &videos);
    assert_eq!(
        (result.added, result.duplicates, result.cancelled),
        (2, 0, false)
    );
    assert_eq!(result.database.tracks[..before.tracks.len()], before.tracks);
    assert_eq!(result.database.playlists[1..], before.playlists[1..]);
    assert_eq!(artwork_files(&fake), before_art);
    assert_eq!(mapping(&fake)["tracks"], before_mapping["tracks"]);
    assert_eq!(mapping(&fake)["playlists"], before_mapping["playlists"]);
    for source in &videos {
        let written = result
            .database
            .tracks
            .iter()
            .find(|track| track.title == source.track.title)
            .unwrap();
        assert_eq!(written.media_kind, IpodMediaKind::Video);
        assert_eq!(written.video_kind, Some(source.kind));
        assert_eq!(written.file_type, "MP4 video file");
        assert_eq!(
            fs::read(&written.path).unwrap(),
            fs::read(&source.track.source_path).unwrap()
        );
    }
    let (plan, after_music) = sync(&fake, &[], &[]);
    assert_eq!(plan.remove.len(), 1, "only owned audio is removed");
    assert_eq!(
        after_music
            .tracks
            .iter()
            .filter(|track| track.title.starts_with("Video "))
            .count(),
        2
    );
    assert_eq!(mapping(&fake)["videoCopies"].as_array().unwrap().len(), 2);
    let planned = plan_video_additions(&opened(&fake), &videos).unwrap();
    assert!(planned.add.is_empty());
    assert_eq!(planned.duplicate, [1, 2]);
    let before_retry = db_bytes(&fake);
    let retry = add(&fake, &videos);
    assert_eq!((retry.added, retry.duplicates), (0, 2));
    assert_eq!(
        db_bytes(&fake),
        before_retry,
        "duplicate-only batches perform no write"
    );
}

#[test]
fn old_mapping_without_video_field_is_read_and_duplicates_within_a_batch_are_skipped() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    sync(&fake, &[], &[]);
    let mut old = mapping(&fake);
    old.as_object_mut().unwrap().remove("videoCopies");
    fs::write(
        itunes_file(&fake, "Unsub.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let one = video(library.path(), 1, VideoKind::Movie);
    let mut duplicate = one.clone();
    duplicate.track.library_id = 2;
    let result = add(&fake, &[one, duplicate]);
    assert_eq!((result.added, result.duplicates), (1, 1));
    assert!(mapping(&fake)["tracks"].as_array().unwrap().is_empty());
}

#[test]
fn cancellation_retains_completed_videos_and_removes_the_partial_file() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let videos = [
        video(library.path(), 1, VideoKind::Movie),
        video(library.path(), 2, VideoKind::Movie),
    ];
    let cancel = AtomicBool::new(false);
    let result = apply_video_additions(&opened(&fake), &videos, &cancel, &mut |progress| {
        if progress.stage == unsub_ipod::SyncStage::Copying && progress.done == 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!((result.added, result.cancelled), (1, true));
    assert_eq!(music_files(&fake).len(), 1);
    assert_eq!(mapping(&fake)["videoCopies"].as_array().unwrap().len(), 1);
    let retry = add(&fake, &videos);
    assert_eq!(
        (retry.added, retry.duplicates, retry.cancelled),
        (1, 1, false)
    );
}

#[test]
fn cancellation_before_any_copy_does_not_rewrite_database_or_mapping() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let one = video(library.path(), 1, VideoKind::Movie);
    let before = db_bytes(&fake);
    let result =
        apply_video_additions(&opened(&fake), &[one], &AtomicBool::new(true), &mut |_| {}).unwrap();
    assert_eq!((result.added, result.cancelled), (0, true));
    assert_eq!(db_bytes(&fake), before);
    assert!(!itunes_file(&fake, "Unsub.json").exists());
}

#[test]
fn fingerprint_mismatch_rolls_back_the_entire_active_batch() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let first = video(library.path(), 1, VideoKind::Movie);
    let mut changed = video(library.path(), 2, VideoKind::Movie);
    changed.sha256 = "0".repeat(64);
    let before = db_bytes(&fake);
    let result = apply_video_additions(
        &opened(&fake),
        &[first, changed],
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("changed since inspection")
    );
    assert_eq!(db_bytes(&fake), before);
    assert!(music_files(&fake).is_empty());
    assert!(!itunes_file(&fake, "Unsub.json").exists());
}

#[test]
fn restoring_a_video_add_removes_only_added_video_and_reconciles_duplicate_mapping() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    // A valid signed backup is required by the existing restore contract.
    sync(&fake, &[], &[]);
    let first = video(library.path(), 1, VideoKind::Movie);
    add(&fake, std::slice::from_ref(&first));
    let second = video(library.path(), 2, VideoKind::MusicVideo);
    add(&fake, std::slice::from_ref(&second));
    restore_backup(&opened(&fake)).unwrap();
    let plan = plan_video_additions(&opened(&fake), &[first, second]).unwrap();
    assert_eq!(plan.duplicate, [1]);
    assert_eq!(plan.add, [2]);
    assert_eq!(music_files(&fake).len(), 1);
}

#[test]
fn missing_copied_video_is_eligible_for_retry_and_corrupt_or_protected_sources_are_refused() {
    let fake = fake_classic();
    let library = tempfile::tempdir().unwrap();
    let one = video(library.path(), 1, VideoKind::Movie);
    add(&fake, std::slice::from_ref(&one));
    let copied = track_path(&fake, "Video 1");
    fs::write(&copied, b"truncated").unwrap();
    assert_eq!(
        plan_video_additions(&opened(&fake), std::slice::from_ref(&one))
            .unwrap()
            .add,
        [1]
    );
    fs::remove_file(copied).unwrap();
    assert_eq!(
        plan_video_additions(&opened(&fake), std::slice::from_ref(&one))
            .unwrap()
            .add,
        [1]
    );
    for entry in [b"encv", b"mp4a"] {
        fs::write(&one.track.source_path, video_bytes(entry, 1)).unwrap();
        assert!(plan_video_additions(&opened(&fake), std::slice::from_ref(&one)).is_err());
    }
}

#[test]
fn video_capability_reuses_model_and_checksum_guards() {
    for (model, expected) in [
        ("A446", true),
        ("C293", true),
        ("A978", true),
        ("B480", true),
        ("C027", false),
        ("A004", false),
        ("C584", false),
    ] {
        let fake = FakeIpod::new("Video capability test");
        fake.write_sysinfo(model, "000A27001A2B3C4D");
        assert_eq!(
            video_add_supported(&opened(&fake)).is_ok(),
            expected,
            "{model}"
        );
    }
    let mut classic = opened(&fake_classic());
    classic.firewire_guid = None;
    assert!(video_add_supported(&classic).is_err());
}

#[test]
fn video_added_beside_legacy_short_headers_keeps_video_classification() {
    let mut db = sample_database();
    db.mhit_header = 0x9c;
    let fake = classic_7g(&db);
    let before = read_database(&opened(&fake)).unwrap();
    let library = tempfile::tempdir().unwrap();
    let result = add(&fake, &[video(library.path(), 1, VideoKind::MusicVideo)]);
    assert_eq!(result.database.tracks[..before.tracks.len()], before.tracks);
    let new = result.database.tracks.last().unwrap();
    assert_eq!(new.video_kind, Some(VideoKind::MusicVideo));
    assert_eq!(new.media_kind, IpodMediaKind::Video);
}
