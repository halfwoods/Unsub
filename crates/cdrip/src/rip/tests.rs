use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use lofty::{
    file::TaggedFileExt,
    picture::{MimeType, PictureType},
    probe::Probe,
    tag::{Accessor, ItemKey, Tag},
};

use super::*;
use crate::{
    ReleaseTrack, VolumeReader,
    testing::{DotNetRandom, FakeClient, make_volume, make_volume_with_audio, synthetic_audio},
};

/// Track lengths in sectors of the standard test disc, as `[start, start, start]` plus lead-out.
const STARTS: [u32; 3] = [150, 170, 184];
const LEAD_OUT: u32 = 200;
const COVER_URL: &str = "https://coverartarchive.org/release/R-1/front-500";

/// A valid 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

struct Fixture {
    dir: tempfile::TempDir,
    disc: Disc,
    audio: Vec<Vec<i16>>,
}

impl Fixture {
    /// A three-track disc of 20, 14 and 16 sectors of synthetic audio.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sectors = [20usize, 14, 16];
        let audio: Vec<Vec<i16>> = (1..)
            .zip(sectors)
            .map(|(seed, sectors)| synthetic_audio(seed, sectors * 588))
            .collect();
        let mount = make_volume_with_audio(dir.path(), "Audio CD", LEAD_OUT, &STARTS, &audio);
        Self {
            disc: VolumeReader::open(&mount).unwrap(),
            dir,
            audio,
        }
    }

    fn reader(&self) -> VolumeReader {
        VolumeReader::new(self.dir.path())
    }

    fn dest(&self) -> PathBuf {
        self.dir.path().join("library")
    }

    fn settings(&self, format: Format) -> RipSettings {
        RipSettings {
            format,
            dest_dir: self.dest(),
            verify_accuraterip: false,
        }
    }

    fn rip(
        &self,
        release: Option<&Release>,
        settings: &RipSettings,
        client: &dyn MetadataClient,
    ) -> Result<Vec<RippedTrack>> {
        rip_with(
            &self.reader(),
            &self.disc,
            release,
            settings,
            client,
            &mut |_| {},
            &AtomicBool::new(false),
        )
    }
}

fn flac(compression: u8) -> Format {
    Format::Flac { compression }
}

fn offline() -> FakeClient {
    FakeClient::default()
}

fn decode_flac(path: &Path) -> Vec<i16> {
    let mut reader = claxon::FlacReader::open(path).unwrap();
    let info = reader.streaminfo();
    assert_eq!(
        (info.sample_rate, info.channels, info.bits_per_sample),
        (44_100, 2, 16)
    );
    let samples: Vec<i16> = reader
        .samples()
        .map(|sample| i16::try_from(sample.unwrap()).unwrap())
        .collect();
    assert_eq!(info.samples, Some(samples.len() as u64 / 2));
    samples
}

fn read_tag(path: &Path) -> Tag {
    Probe::open(path)
        .unwrap()
        .read()
        .unwrap()
        .primary_tag()
        .unwrap()
        .clone()
}

fn release() -> Release {
    let track = |position: u32, title: &str, artist: &str| ReleaseTrack {
        position,
        title: title.to_owned(),
        artist: artist.to_owned(),
        recording_mbid: Some(format!("recording-{position}")),
        track_mbid: Some(format!("track-{position}")),
        artist_mbid: Some(format!("artist-{position}")),
        length_ms: None,
    };
    Release {
        mbid: "R-1".into(),
        title: "Abbey Road".into(),
        artist: "The Beatles".into(),
        year: Some(1969),
        disc_number: 1,
        disc_count: 2,
        tracks: vec![
            track(1, "Come Together", "The Beatles"),
            track(2, "Something", "George Harrison"),
            track(3, "Maxwell's Silver Hammer", "The Beatles"),
        ],
        cover_art_url: Some(COVER_URL.into()),
        release_group_mbid: Some("RG-1".into()),
        artist_mbid: Some("album-artist-1".into()),
    }
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn flac_rip_is_bit_exact_and_named_without_a_release() {
    let fixture = Fixture::new();
    let ripped = fixture
        .rip(None, &fixture.settings(flac(5)), &offline())
        .unwrap();
    let names: Vec<_> = ripped
        .iter()
        .map(|track| track.path.strip_prefix(fixture.dest()).unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            Path::new("Unknown Artist/Unknown Album/1-01 Track 01.flac"),
            Path::new("Unknown Artist/Unknown Album/1-02 Track 02.flac"),
            Path::new("Unknown Artist/Unknown Album/1-03 Track 03.flac"),
        ]
    );
    for (track, original) in ripped.iter().zip(&fixture.audio) {
        assert_eq!(
            &decode_flac(&track.path),
            original,
            "track {}",
            track.number
        );
        assert_eq!(track.accuraterip, AccurateRip::Skipped);
    }
    assert_eq!(
        ripped.iter().map(|track| track.number).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn every_compression_level_is_lossless() {
    let fixture = Fixture::new();
    for level in 0..=9 {
        let dir = fixture.dir.path().join(format!("level-{level}"));
        let settings = RipSettings {
            dest_dir: dir,
            ..fixture.settings(flac(level))
        };
        let ripped = fixture.rip(None, &settings, &offline()).unwrap();
        for (track, original) in ripped.iter().zip(&fixture.audio) {
            assert_eq!(
                &decode_flac(&track.path),
                original,
                "level {level}, track {}",
                track.number
            );
        }
    }
}

#[test]
fn higher_levels_do_not_make_music_bigger() {
    let fixture = Fixture::new();
    let size = |level| {
        let settings = RipSettings {
            dest_dir: fixture.dir.path().join(format!("size-{level}")),
            ..fixture.settings(flac(level))
        };
        let ripped = fixture.rip(None, &settings, &offline()).unwrap();
        ripped
            .iter()
            .map(|track| fs::metadata(&track.path).unwrap().len())
            .sum::<u64>()
    };
    assert!(size(8) <= size(0));
    assert!(size(0) < fixture.audio.iter().map(|a| a.len() as u64 * 2).sum());
}

#[test]
fn release_metadata_names_the_files_and_fills_the_tags() {
    let fixture = Fixture::new();
    let client = offline().with(COVER_URL, PNG);
    let ripped = fixture
        .rip(Some(&release()), &fixture.settings(flac(5)), &client)
        .unwrap();

    let second = &ripped[1].path;
    assert_eq!(
        second.strip_prefix(fixture.dest()).unwrap(),
        Path::new("The Beatles/Abbey Road/1-02 Something.flac")
    );
    assert_eq!(client.requested(), [COVER_URL]);

    let tag = read_tag(second);
    assert_eq!(tag.title().as_deref(), Some("Something"));
    assert_eq!(tag.artist().as_deref(), Some("George Harrison"));
    assert_eq!(tag.album().as_deref(), Some("Abbey Road"));
    assert_eq!(tag.get_string(ItemKey::AlbumArtist), Some("The Beatles"));
    assert_eq!((tag.track(), tag.track_total()), (Some(2), Some(3)));
    assert_eq!((tag.disk(), tag.disk_total()), (Some(1), Some(2)));
    assert_eq!(tag.get_string(ItemKey::RecordingDate), Some("1969"));
    assert_eq!(tag.get_string(ItemKey::MusicBrainzReleaseId), Some("R-1"));
    assert_eq!(
        tag.get_string(ItemKey::MusicBrainzReleaseGroupId),
        Some("RG-1")
    );
    assert_eq!(
        tag.get_string(ItemKey::MusicBrainzRecordingId),
        Some("recording-2")
    );
    assert_eq!(tag.get_string(ItemKey::MusicBrainzTrackId), Some("track-2"));
    assert_eq!(
        tag.get_string(ItemKey::MusicBrainzArtistId),
        Some("artist-2")
    );
    assert_eq!(
        tag.get_string(ItemKey::MusicBrainzReleaseArtistId),
        Some("album-artist-1")
    );

    let pictures = tag.pictures();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].pic_type(), PictureType::CoverFront);
    assert_eq!(pictures[0].mime_type(), Some(&MimeType::Png));
    assert_eq!(pictures[0].data(), PNG);

    // Tagging must not disturb the audio.
    assert_eq!(decode_flac(second), fixture.audio[1]);
}

#[test]
fn without_a_release_tracks_still_get_numbers_and_placeholder_names() {
    let fixture = Fixture::new();
    let ripped = fixture
        .rip(None, &fixture.settings(flac(5)), &offline())
        .unwrap();
    let tag = read_tag(&ripped[1].path);
    assert_eq!(tag.title().as_deref(), Some("Track 02"));
    assert_eq!(tag.artist().as_deref(), Some("Unknown Artist"));
    assert_eq!(tag.album().as_deref(), Some("Unknown Album"));
    assert_eq!(
        (tag.track(), tag.track_total(), tag.disk(), tag.disk_total()),
        (Some(2), Some(3), Some(1), Some(1))
    );
    assert!(tag.pictures().is_empty());
}

#[test]
fn a_named_volume_is_used_as_the_album_when_there_is_no_release() {
    let dir = tempfile::tempdir().unwrap();
    let audio = vec![synthetic_audio(1, 20 * 588)];
    let mount = make_volume_with_audio(dir.path(), "Kind of Blue", 170, &[150], &audio);
    let disc = VolumeReader::open(&mount).unwrap();
    let settings = RipSettings {
        format: flac(5),
        dest_dir: dir.path().join("out"),
        verify_accuraterip: false,
    };
    let ripped = rip_with(
        &VolumeReader::new(dir.path()),
        &disc,
        None,
        &settings,
        &offline(),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        ripped[0]
            .path
            .ends_with("Unknown Artist/Kind of Blue/1-01 Track 01.flac")
    );
}

#[test]
fn the_second_disc_of_a_set_is_numbered_as_such() {
    let fixture = Fixture::new();
    let second_disc = Release {
        disc_number: 2,
        ..release()
    };
    let ripped = fixture
        .rip(
            Some(&second_disc),
            &fixture.settings(flac(5)),
            &offline().without(COVER_URL),
        )
        .unwrap();
    assert!(
        ripped[0]
            .path
            .ends_with("The Beatles/Abbey Road/2-01 Come Together.flac")
    );
    assert_eq!(read_tag(&ripped[0].path).disk(), Some(2));
}

#[test]
fn tracks_the_release_does_not_list_fall_back_to_placeholders() {
    let fixture = Fixture::new();
    let mut short = release();
    short.tracks.truncate(2);
    short.cover_art_url = None;
    let ripped = fixture
        .rip(Some(&short), &fixture.settings(flac(5)), &offline())
        .unwrap();
    assert!(
        ripped[1]
            .path
            .ends_with("The Beatles/Abbey Road/1-02 Something.flac")
    );
    assert!(
        ripped[2]
            .path
            .ends_with("The Beatles/Abbey Road/1-03 Track 03.flac")
    );
    let tag = read_tag(&ripped[2].path);
    assert_eq!(
        (tag.title().as_deref(), tag.artist().as_deref()),
        (Some("Track 03"), Some("The Beatles"))
    );
    assert_eq!(tag.get_string(ItemKey::MusicBrainzTrackId), None);
}

#[test]
fn hostile_metadata_cannot_escape_the_destination() {
    let fixture = Fixture::new();
    let mut hostile = release();
    hostile.artist = "../../outside".into();
    hostile.title = "C:\\Windows".into();
    hostile.tracks[0].title = "../../../etc/passwd".into();
    hostile.cover_art_url = None;
    let ripped = fixture
        .rip(Some(&hostile), &fixture.settings(flac(5)), &offline())
        .unwrap();
    for track in &ripped {
        assert!(
            track.path.starts_with(fixture.dest()),
            "{}",
            track.path.display()
        );
        assert_eq!(
            track
                .path
                .strip_prefix(fixture.dest())
                .unwrap()
                .components()
                .count(),
            3
        );
    }
}

#[test]
fn ripping_again_replaces_the_files() {
    let fixture = Fixture::new();
    let first = fixture
        .rip(None, &fixture.settings(flac(5)), &offline())
        .unwrap();
    fs::write(&first[0].path, b"stale").unwrap();
    let second = fixture
        .rip(None, &fixture.settings(flac(5)), &offline())
        .unwrap();
    assert_eq!(first[0].path, second[0].path);
    assert_eq!(decode_flac(&second[0].path), fixture.audio[0]);
    assert_eq!(files_under(&fixture.dest()).len(), 3);
}

#[test]
fn no_temporary_files_are_left_after_a_successful_rip() {
    let fixture = Fixture::new();
    fixture
        .rip(
            Some(&release()),
            &fixture.settings(flac(5)),
            &offline().with(COVER_URL, PNG),
        )
        .unwrap();
    for file in files_under(&fixture.dest()) {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            !name.starts_with('.') && !name.ends_with(".partial"),
            "{name}"
        );
    }
}

#[test]
fn progress_reports_each_track_in_order() {
    let fixture = Fixture::new();
    let mut events = Vec::new();
    rip_with(
        &fixture.reader(),
        &fixture.disc,
        None,
        &fixture.settings(flac(5)),
        &offline(),
        &mut |event| events.push(event),
        &AtomicBool::new(false),
    )
    .unwrap();

    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RipProgress::TrackStarted {
                number,
                index,
                count,
            } => Some((*number, *index, *count)),
            _ => None,
        })
        .collect();
    assert_eq!(started, [(1, 0, 3), (2, 1, 3), (3, 2, 3)]);

    let mut current = 0;
    let mut last_fraction = 0.0;
    for event in &events {
        match event {
            RipProgress::TrackStarted { number, .. } => {
                current = *number;
                last_fraction = 0.0;
            }
            RipProgress::TrackProgress { number, fraction } => {
                assert_eq!(*number, current);
                assert!(*fraction >= last_fraction && *fraction <= 1.0);
                last_fraction = *fraction;
            }
            RipProgress::TrackFinished(track) => assert_eq!(track.number, current),
        }
    }
    assert!(matches!(events.last(), Some(RipProgress::TrackFinished(track)) if track.number == 3));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RipProgress::TrackProgress { .. }))
    );
}

#[test]
fn a_failed_cover_download_costs_only_the_cover() {
    for client in [
        offline().without(COVER_URL),
        offline(),
        offline().with(COVER_URL, b"<html>not an image</html>"),
    ] {
        let fixture = Fixture::new();
        let ripped = fixture
            .rip(Some(&release()), &fixture.settings(flac(5)), &client)
            .unwrap();
        assert_eq!(ripped.len(), 3);
        assert!(read_tag(&ripped[0].path).pictures().is_empty());
        assert_eq!(decode_flac(&ripped[0].path), fixture.audio[0]);
    }
}

#[test]
fn cancelling_before_the_first_track_writes_nothing() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(true);
    let result = rip_with(
        &fixture.reader(),
        &fixture.disc,
        None,
        &fixture.settings(flac(5)),
        &offline(),
        &mut |_| {},
        &cancel,
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(!fixture.dest().exists() || files_under(&fixture.dest()).is_empty());
}

#[test]
fn cancelling_inside_the_first_track_leaves_nothing_behind() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut on_progress = |event: RipProgress| {
        if matches!(event, RipProgress::TrackProgress { .. }) {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let result = rip_with(
        &fixture.reader(),
        &fixture.disc,
        None,
        &fixture.settings(flac(5)),
        &offline(),
        &mut on_progress,
        &cancel,
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(
        files_under(&fixture.dest()).is_empty(),
        "{:?}",
        files_under(&fixture.dest())
    );
    // Not even the empty artist and album folders.
    assert_eq!(fs::read_dir(fixture.dest()).map_or(0, Iterator::count), 0);
}

#[test]
fn cancelling_mid_rip_keeps_finished_tracks_and_leaves_no_partial_file() {
    let fixture = Fixture::new();
    let cancel = AtomicBool::new(false);
    let mut finished = Vec::new();
    let mut on_progress = |event: RipProgress| match event {
        RipProgress::TrackFinished(track) => finished.push(track),
        RipProgress::TrackProgress { number: 2, .. } => cancel.store(true, Ordering::Relaxed),
        _ => {}
    };
    let result = rip_with(
        &fixture.reader(),
        &fixture.disc,
        None,
        &fixture.settings(flac(5)),
        &offline(),
        &mut on_progress,
        &cancel,
    );
    assert!(matches!(result, Err(Error::Cancelled)));

    assert_eq!(finished.len(), 1);
    let on_disk = files_under(&fixture.dest());
    assert_eq!(on_disk, [finished[0].path.clone()]);
    assert_eq!(decode_flac(&on_disk[0]), fixture.audio[0]);
}

#[test]
fn an_audio_file_that_ends_early_fails_the_rip_without_a_partial_file() {
    let fixture = Fixture::new();
    let broken = &fixture.disc.tracks[1].path;
    let mut bytes = fs::read(broken).unwrap();
    bytes.truncate(bytes.len() - 5_000);
    fs::write(broken, bytes).unwrap();

    let result = fixture.rip(None, &fixture.settings(flac(5)), &offline());
    assert!(matches!(result, Err(Error::Disc(_))), "{result:?}");
    let on_disk = files_under(&fixture.dest());
    assert_eq!(on_disk.len(), 1, "{on_disk:?}");
    assert!(on_disk[0].ends_with("1-01 Track 01.flac"));
}

#[test]
fn a_disc_without_audio_tracks_is_refused() {
    let mut fixture = Fixture::new();
    fixture.disc.tracks.clear();
    assert!(matches!(
        fixture.rip(None, &fixture.settings(flac(5)), &offline()),
        Err(Error::Disc(_))
    ));
}

/// The CUETools reference disc: a 13 sector pregap (not on the volume) and tracks of 55, 31 and 37
/// sectors made from `Random(2314)`, whose v1/v2 checksums its own tests pin down.
mod accuraterip_disc {
    use super::*;

    const URL: &str =
        "https://www.accuraterip.com/accuraterip/c/3/1/dBAR-003-0000013c-000003de-07000103.bin";
    const IDS: (u32, u32, u32) = (0x13c, 0x3de, 0x0700_0103);
    const V1: [u32; 3] = [3_727_147_246, 2_202_235_240, 3_752_629_998];
    const V2: [u32; 3] = [3_988_391_122, 2_284_845_104, 3_841_231_027];

    fn fixture() -> Fixture {
        let mut random = DotNetRandom::new(2314);
        let _pregap = random.samples(13 * 588);
        let audio: Vec<Vec<i16>> = [55usize, 31, 37]
            .iter()
            .map(|sectors| random.samples(sectors * 588))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let mount = make_volume_with_audio(
            dir.path(),
            "Audio CD",
            150 + 136,
            &[150 + 13, 150 + 68, 150 + 99],
            &audio,
        );
        Fixture {
            disc: VolumeReader::open(&mount).unwrap(),
            dir,
            audio,
        }
    }

    fn pressing(tracks: &[(u8, u32)]) -> Vec<u8> {
        let mut bytes = vec![tracks.len() as u8];
        for id in [IDS.0, IDS.1, IDS.2] {
            bytes.extend(id.to_le_bytes());
        }
        for &(confidence, checksum) in tracks {
            bytes.push(confidence);
            bytes.extend(checksum.to_le_bytes());
            bytes.extend(0u32.to_le_bytes());
        }
        bytes
    }

    fn verified(fixture: &Fixture, client: &FakeClient) -> Vec<AccurateRip> {
        let settings = RipSettings {
            verify_accuraterip: true,
            ..fixture.settings(flac(2))
        };
        fixture
            .rip(None, &settings, client)
            .unwrap()
            .into_iter()
            .map(|track| track.accuraterip)
            .collect()
    }

    #[test]
    fn matching_checksums_report_the_best_confidence_across_pressings_and_versions() {
        let fixture = fixture();
        let database = [
            pressing(&[(12, V1[0]), (5, V1[1]), (30, V2[2])]),
            pressing(&[(2, 1), (9, V2[1]), (4, 3)]),
        ]
        .concat();
        let client = offline().with(URL, &database);
        assert_eq!(
            verified(&fixture, &client),
            [
                AccurateRip::Matched { confidence: 12 },
                AccurateRip::Matched { confidence: 9 },
                AccurateRip::Matched { confidence: 30 },
            ]
        );
        assert_eq!(client.requested(), [URL]);
    }

    #[test]
    fn a_checksum_unlike_any_pressing_is_a_mismatch_per_track() {
        let fixture = fixture();
        let database = pressing(&[(12, V1[0]), (5, 0xdead_beef), (30, 7)]);
        assert_eq!(
            verified(&fixture, &offline().with(URL, &database)),
            [
                AccurateRip::Matched { confidence: 12 },
                AccurateRip::Mismatch,
                AccurateRip::Mismatch
            ]
        );
    }

    #[test]
    fn a_disc_missing_from_the_database_is_reported_as_such() {
        let fixture = fixture();
        assert_eq!(
            verified(&fixture, &offline().without(URL)),
            [AccurateRip::NotInDatabase; 3]
        );
    }

    #[test]
    fn being_offline_skips_verification_but_not_the_rip() {
        let fixture = fixture();
        assert_eq!(verified(&fixture, &offline()), [AccurateRip::Skipped; 3]);
        assert_eq!(files_under(&fixture.dest()).len(), 3);
    }

    #[test]
    fn a_corrupt_database_skips_verification() {
        let fixture = fixture();
        assert_eq!(
            verified(&fixture, &offline().with(URL, &[3, 1, 2])),
            [AccurateRip::Skipped; 3]
        );
    }

    #[test]
    fn verification_off_never_asks_the_database() {
        let fixture = fixture();
        let client = offline();
        let ripped = fixture
            .rip(None, &fixture.settings(flac(2)), &client)
            .unwrap();
        assert!(
            ripped
                .iter()
                .all(|track| track.accuraterip == AccurateRip::Skipped)
        );
        assert!(client.requested().is_empty());
    }

    #[test]
    fn the_audio_is_unchanged_by_verification() {
        let fixture = fixture();
        let database = pressing(&[(1, V1[0]), (1, V1[1]), (1, V1[2])]);
        let settings = RipSettings {
            verify_accuraterip: true,
            ..fixture.settings(flac(2))
        };
        let ripped = fixture
            .rip(None, &settings, &offline().with(URL, &database))
            .unwrap();
        for (track, original) in ripped.iter().zip(&fixture.audio) {
            assert_eq!(&decode_flac(&track.path), original);
        }
    }
}

#[test]
fn discs_with_data_tracks_skip_accuraterip() {
    let dir = tempfile::tempdir().unwrap();
    let mount = make_volume(
        dir.path(),
        "Mixed",
        1,
        1_000,
        &[150, 400, 800],
        &[false, false, true],
    );
    let disc = VolumeReader::open(&mount).unwrap();
    let settings = RipSettings {
        format: flac(2),
        dest_dir: dir.path().join("out"),
        verify_accuraterip: true,
    };
    let client = offline();
    let ripped = rip_with(
        &VolumeReader::new(dir.path()),
        &disc,
        None,
        &settings,
        &client,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(ripped.len(), 2);
    assert!(
        ripped
            .iter()
            .all(|track| track.accuraterip == AccurateRip::Skipped)
    );
    assert!(client.requested().is_empty());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn this_platform_cannot_read_discs_yet() {
    let fixture = Fixture::new();
    let result = rip(
        &fixture.disc,
        None,
        &fixture.settings(flac(5)),
        &offline(),
        &mut |_| {},
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(Error::Unsupported(_))));
    assert!(crate::discs().is_empty());
    assert!(matches!(
        crate::eject(&fixture.disc),
        Err(Error::Unsupported(_))
    ));
}

/// ALAC and AAC go through AudioToolbox, which only exists on macOS.
#[cfg(target_os = "macos")]
mod audiotoolbox {
    use super::*;
    use crate::testing::decode_m4a;

    fn rms(samples: &[i16]) -> f64 {
        (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[test]
    fn alac_rip_decodes_bit_exactly_and_carries_the_tags_and_cover() {
        let fixture = Fixture::new();
        let client = offline().with(COVER_URL, PNG);
        let ripped = fixture
            .rip(Some(&release()), &fixture.settings(Format::Alac), &client)
            .unwrap();

        assert_eq!(
            ripped[1].path.strip_prefix(fixture.dest()).unwrap(),
            Path::new("The Beatles/Abbey Road/1-02 Something.m4a")
        );
        for (track, original) in ripped.iter().zip(&fixture.audio) {
            assert_eq!(&decode_m4a(&track.path), original, "track {}", track.number);
        }

        let tag = read_tag(&ripped[1].path);
        assert_eq!(tag.title().as_deref(), Some("Something"));
        assert_eq!(tag.artist().as_deref(), Some("George Harrison"));
        assert_eq!(tag.album().as_deref(), Some("Abbey Road"));
        assert_eq!(tag.get_string(ItemKey::AlbumArtist), Some("The Beatles"));
        assert_eq!(
            (tag.track(), tag.track_total(), tag.disk(), tag.disk_total()),
            (Some(2), Some(3), Some(1), Some(2))
        );
        assert_eq!(tag.get_string(ItemKey::RecordingDate), Some("1969"));
        assert_eq!(tag.get_string(ItemKey::MusicBrainzReleaseId), Some("R-1"));
        assert_eq!(tag.get_string(ItemKey::MusicBrainzTrackId), Some("track-2"));
        assert_eq!(tag.pictures().len(), 1);
        assert_eq!(tag.pictures()[0].data(), PNG);
        // Tagging rewrote the file; it must still decode, which the first loop already proved only
        // for the tagged copies.
        assert_eq!(files_under(&fixture.dest()).len(), 3);
    }

    #[test]
    fn aac_rip_is_lossy_but_faithful_and_follows_the_bitrate() {
        let dir = tempfile::tempdir().unwrap();
        // Long enough for the bitrate to show in the file size.
        let audio = vec![
            synthetic_audio(1, 44_100 * 3),
            synthetic_audio(2, 44_100 * 3),
        ];
        let mount = make_volume_with_audio(
            dir.path(),
            "Audio CD",
            150 + 2 * 1_800,
            &[150, 150 + 1_800],
            &audio,
        );
        let disc = VolumeReader::open(&mount).unwrap();

        let rip_at = |kbps: u32| {
            let settings = RipSettings {
                format: Format::Aac { kbps },
                dest_dir: dir.path().join(format!("aac-{kbps}")),
                verify_accuraterip: false,
            };
            rip_with(
                &VolumeReader::new(dir.path()),
                &disc,
                None,
                &settings,
                &offline(),
                &mut |_| {},
                &AtomicBool::new(false),
            )
            .unwrap()
        };
        let low = rip_at(64);
        let high = rip_at(256);

        let size = |ripped: &[RippedTrack]| fs::metadata(&ripped[0].path).unwrap().len();
        assert!(
            size(&high) > size(&low) * 2,
            "{} vs {}",
            size(&high),
            size(&low)
        );
        // About 3 s: 24 kB at 64 kbps and 96 kB at 256 kbps, plus a few kB of container. A fat
        // header reservation would blow through these.
        assert!((24_000..34_000).contains(&size(&low)), "{}", size(&low));
        assert!((96_000..112_000).contains(&size(&high)), "{}", size(&high));

        let decoded = decode_m4a(&high[0].path);
        // Encoder delay and padding shift the length by a couple of packets at most.
        assert!(
            decoded.len().abs_diff(audio[0].len()) < 2 * 4096,
            "{} vs {}",
            decoded.len(),
            audio[0].len()
        );
        let ratio = rms(&decoded) / rms(&audio[0]);
        assert!((0.8..1.2).contains(&ratio), "{ratio}");
        assert_eq!(read_tag(&high[0].path).title().as_deref(), Some("Track 01"));
    }

    #[test]
    fn an_unusable_aac_bitrate_is_refused_before_anything_is_written() {
        let fixture = Fixture::new();
        for kbps in [0, 8, 1_000] {
            let result = fixture.rip(None, &fixture.settings(Format::Aac { kbps }), &offline());
            assert!(
                matches!(result, Err(Error::Encode(_))),
                "{kbps}: {result:?}"
            );
        }
        assert!(files_under(&fixture.dest()).is_empty());
    }

    #[test]
    fn cancelling_an_alac_rip_removes_the_partial_file() {
        let fixture = Fixture::new();
        let cancel = AtomicBool::new(false);
        let mut on_progress = |event: RipProgress| {
            if matches!(event, RipProgress::TrackProgress { number: 2, .. }) {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let result = rip_with(
            &fixture.reader(),
            &fixture.disc,
            None,
            &fixture.settings(Format::Alac),
            &offline(),
            &mut on_progress,
            &cancel,
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        let on_disk = files_under(&fixture.dest());
        assert_eq!(on_disk.len(), 1, "{on_disk:?}");
        assert!(on_disk[0].ends_with("1-01 Track 01.m4a"));
    }

    #[test]
    fn accuraterip_checks_the_source_samples_whatever_the_codec() {
        // Verification runs on what was read from the disc, not on the encoded file, so it
        // behaves the same for a lossy format.
        let mut random = DotNetRandom::new(2314);
        let _pregap = random.samples(13 * 588);
        let audio: Vec<Vec<i16>> = [55usize, 31, 37]
            .iter()
            .map(|sectors| random.samples(sectors * 588))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let mount = make_volume_with_audio(
            dir.path(),
            "Audio CD",
            150 + 136,
            &[150 + 13, 150 + 68, 150 + 99],
            &audio,
        );
        let fixture = Fixture {
            disc: VolumeReader::open(&mount).unwrap(),
            dir,
            audio,
        };

        let url =
            "https://www.accuraterip.com/accuraterip/c/3/1/dBAR-003-0000013c-000003de-07000103.bin";
        let mut database = vec![3u8];
        for id in [0x13cu32, 0x3de, 0x0700_0103] {
            database.extend(id.to_le_bytes());
        }
        for checksum in [3_727_147_246u32, 2_202_235_240, 3_752_629_998] {
            database.push(7);
            database.extend(checksum.to_le_bytes());
            database.extend(0u32.to_le_bytes());
        }
        let settings = RipSettings {
            verify_accuraterip: true,
            ..fixture.settings(Format::Aac { kbps: 128 })
        };
        let ripped = fixture
            .rip(None, &settings, &offline().with(url, &database))
            .unwrap();
        assert!(
            ripped
                .iter()
                .all(|track| track.accuraterip == AccurateRip::Matched { confidence: 7 })
        );
    }
}
