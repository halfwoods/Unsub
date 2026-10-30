use super::*;
use crate::common::artwork::png;

const CLASSIC_IMAGE_BYTES: u64 = 2 * (56 * 56 + 128 * 128 + 128 * 128 + 320 * 320);
const VIDEO_IMAGE_BYTES: u64 = 2 * (100 * 100 + 200 * 200);

fn check_space(model: &str, tracks: &[SyncTrack], expected_artwork: u64) {
    let fake = FakeIpod::new("ARTWORK SPACE");
    fake.write_itunesdb(&sample_database().build());
    fake.write_sysinfo(model, "0x000A27001A2B3C4D");
    let music_bytes: u64 = tracks
        .iter()
        .map(|track| fs::metadata(&track.source_path).unwrap().len())
        .sum();

    let (planned, _) = sync(&fake, tracks, &[]);
    let appended: u64 = fs::read_dir(fake.mount.join("iPod_Control/Artwork"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|file| {
            file.extension()
                .is_some_and(|extension| extension == "ithmb")
        })
        .map(|file| fs::metadata(file).unwrap().len())
        .sum();
    assert_eq!(appended, expected_artwork, "independent format-size bound");
    assert_eq!(planned.bytes_needed, music_bytes + appended);
}

#[test]
fn sparse_album_with_distinct_song_covers_reserves_every_image() {
    let library = tempfile::tempdir().unwrap();
    let red = png(32, 32, [255, 0, 0]);
    let blue = png(32, 32, [0, 0, 255]);
    let tracks = [
        artwork::covered(library.path(), 1, "One", red.clone()),
        artwork::covered(library.path(), 2, "Two", blue),
        artwork::covered(library.path(), 3, "Three", red),
    ];
    check_space("xC293", &tracks, 2 * CLASSIC_IMAGE_BYTES);
}

#[test]
fn sparse_identical_covers_in_different_albums_are_separate_images() {
    let library = tempfile::tempdir().unwrap();
    let red = png(32, 32, [255, 0, 0]);
    let mut tracks = [
        artwork::covered(library.path(), 1, "One", red.clone()),
        artwork::covered(library.path(), 2, "Two", red),
    ];
    tracks[1].album = "Another album".into();
    check_space("xC293", &tracks, 2 * CLASSIC_IMAGE_BYTES);
}

#[test]
fn nonsparse_tracks_with_identical_covers_each_reserve_an_image() {
    let library = tempfile::tempdir().unwrap();
    let red = png(32, 32, [255, 0, 0]);
    let tracks = [
        artwork::covered(library.path(), 1, "One", red.clone()),
        artwork::covered(library.path(), 2, "Two", red),
    ];
    check_space("xA448", &tracks, 2 * VIDEO_IMAGE_BYTES);
}
