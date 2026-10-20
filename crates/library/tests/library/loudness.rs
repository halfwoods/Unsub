use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::id3::v2::{CommentFrame, Frame, Id3v2Tag};
use lofty::iff::aiff::AiffFile;
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst, Mp4File};
use lofty::tag::{ItemKey, Tag, TagType};
use rusqlite::Connection;
use tempfile::TempDir;
use unsub_library::{
    Gain, GainMode, Library, Loudness, LoudnessSource, MediaKind, Track, TrackEdits, TrackLoudness,
    TrackOverrides, itunes_soundcheck,
};

use crate::common::{
    added_ids, all_kinds, copy_fixtures, library_of, rewind_schema, scan, scanned_library, track,
    track_titled,
};

/// The sample from id3.org's description of the iTunNORM comment.
const ITUNNORM_SAMPLE: &str =
    " 00001E86 00001E86 0000A2A3 0000A2A3 000006A6 000006A6 000078FA 000078FA 00000211 00000211";
/// 8.929 dB of cut from 0x1E86 = 7814: -18 LUFS + 8.929.
const ITUNNORM_LUFS: f64 = -9.0709;

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 0.01, "{actual} vs {expected}");
}

fn retag(path: &Path, change: impl FnOnce(&mut Tag)) {
    let mut tagged = lofty::read_from_path(path).unwrap();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tagged.primary_tag_type()));
    }
    change(tagged.primary_tag_mut().unwrap());
    tagged.save_to_path(path, WriteOptions::default()).unwrap();
}

fn replaygain(
    tag: &mut Tag,
    track_gain: &str,
    track_peak: &str,
    album_gain: &str,
    album_peak: &str,
) {
    tag.insert_text(ItemKey::ReplayGainTrackGain, track_gain.into());
    tag.insert_text(ItemKey::ReplayGainTrackPeak, track_peak.into());
    tag.insert_text(ItemKey::ReplayGainAlbumGain, album_gain.into());
    tag.insert_text(ItemKey::ReplayGainAlbumPeak, album_peak.into());
}

fn loudness_of(library: &Library, title: &str) -> Option<TrackLoudness> {
    track_titled(library, title).loudness
}

fn add_itunnorm_to_mp3(path: &Path, text: &str) {
    let mut mpeg =
        lofty::mpeg::MpegFile::read_from(&mut fs::File::open(path).unwrap(), ParseOptions::new())
            .unwrap();
    mpeg.id3v2_mut()
        .unwrap()
        .insert(Frame::Comment(CommentFrame::new(
            TextEncoding::Latin1,
            *b"eng",
            "iTunNORM",
            text.to_owned(),
        )));
    mpeg.save_to_path(path, WriteOptions::default()).unwrap();
}

fn add_itunnorm_to_mp4(path: &Path, text: &str) {
    let mut mp4 =
        Mp4File::read_from(&mut fs::File::open(path).unwrap(), ParseOptions::new()).unwrap();
    if mp4.ilst().is_none() {
        mp4.set_ilst(Ilst::new());
    }
    mp4.ilst_mut().unwrap().insert(Atom::new(
        AtomIdent::Freeform {
            mean: "com.apple.iTunes".into(),
            name: "iTunNORM".into(),
        },
        AtomData::UTF8(text.to_owned()),
    ));
    mp4.save_to_path(path, WriteOptions::default()).unwrap();
}

/// A tenth of a second of silence in an AIFF container, as iTunes' CD import
/// makes, so Sound Check data in its ID3 tag can be tested.
fn write_aiff(path: &Path, itunnorm: &str) {
    let frames: u32 = 800;
    let mut comm = Vec::new();
    comm.extend(1u16.to_be_bytes());
    comm.extend(frames.to_be_bytes());
    comm.extend(16u16.to_be_bytes());
    comm.extend([0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0]); // 8000 Hz as an 80-bit float
    let mut ssnd = Vec::new();
    ssnd.extend(0u32.to_be_bytes());
    ssnd.extend(0u32.to_be_bytes());
    ssnd.extend(vec![0u8; frames as usize * 2]);
    let mut form = b"AIFF".to_vec();
    for (name, body) in [(b"COMM", &comm), (b"SSND", &ssnd)] {
        form.extend(name);
        form.extend((body.len() as u32).to_be_bytes());
        form.extend(body.iter());
    }
    let mut bytes = b"FORM".to_vec();
    bytes.extend((form.len() as u32).to_be_bytes());
    bytes.extend(form);
    fs::write(path, bytes).unwrap();

    let mut aiff =
        AiffFile::read_from(&mut fs::File::open(path).unwrap(), ParseOptions::new()).unwrap();
    let mut tag = Id3v2Tag::new();
    tag.insert(Frame::Comment(CommentFrame::new(
        TextEncoding::Latin1,
        *b"eng",
        "iTunNORM",
        itunnorm.to_owned(),
    )));
    aiff.set_id3v2(tag);
    aiff.save_to_path(path, WriteOptions::default()).unwrap();
}

#[test]
fn replaygain_tags_are_read_from_every_container() {
    let dir = copy_fixtures();
    for file in [
        "come_together.flac",
        "halo.mp3",
        "something.m4a",
        "zombie.ogg",
    ] {
        retag(&dir.path().join(file), |tag| {
            replaygain(tag, "-6.50 dB", "0.988553", "-7.25 dB", "1.000000")
        });
    }
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    for title in ["Come Together", "Halo", "Something", "Zombie"] {
        let loudness = loudness_of(&library, title).unwrap_or_else(|| panic!("{title}"));
        assert_eq!(loudness.source, LoudnessSource::ReplayGain, "{title}");
        close(loudness.lufs, -11.5);
        close(loudness.true_peak_dbtp.unwrap(), -0.1005);
        close(loudness.album_lufs.unwrap(), -10.75);
        close(loudness.album_true_peak_dbtp.unwrap(), 0.0);
    }
    assert!(
        loudness_of(&library, "Bohemian Rhapsody").is_none(),
        "untagged files have none"
    );
}

#[test]
fn replaygain_in_a_secondary_tag_is_found() {
    let dir = copy_fixtures();
    let path = dir.path().join("halo.mp3");
    let mut tagged = lofty::read_from_path(&path).unwrap();
    let mut ape = Tag::new(TagType::Ape);
    replaygain(&mut ape, "+2.00 dB", "0.5", "+1.00 dB", "0.6");
    tagged.insert_tag(ape);
    tagged.save_to_path(&path, WriteOptions::default()).unwrap();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let loudness = loudness_of(&library, "Halo").unwrap();

    close(loudness.lufs, -20.0);
    close(loudness.true_peak_dbtp.unwrap(), -6.0206);
    close(loudness.album_lufs.unwrap(), -19.0);
    assert_eq!(
        track_titled(&library, "Halo").genre,
        "Pop",
        "the ID3 tag is still the main one"
    );
}

#[test]
fn r128_tags_are_read_in_q7_8_against_minus_23_lufs() {
    let dir = copy_fixtures();
    retag(&dir.path().join("zombie.ogg"), |tag| {
        tag.insert_text(ItemKey::R128TrackGain, "-573".into());
        tag.insert_text(ItemKey::R128AlbumGain, "256".into());
    });
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let loudness = loudness_of(&library, "Zombie").unwrap();

    assert_eq!(loudness.source, LoudnessSource::R128Tag);
    close(loudness.lufs, -20.7617);
    close(loudness.album_lufs.unwrap(), -24.0);
    assert_eq!(
        (loudness.true_peak_dbtp, loudness.album_true_peak_dbtp),
        (None, None),
        "R128 tags carry no peak"
    );
}

#[test]
fn itunnorm_is_read_from_id3_comments_aiff_and_mp4() {
    let dir = copy_fixtures();
    add_itunnorm_to_mp3(&dir.path().join("halo.mp3"), ITUNNORM_SAMPLE);
    add_itunnorm_to_mp4(&dir.path().join("something.m4a"), ITUNNORM_SAMPLE);
    write_aiff(&dir.path().join("cd rip.aiff"), ITUNNORM_SAMPLE);
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    for title in ["Halo", "Something", "cd rip"] {
        let loudness = loudness_of(&library, title).unwrap_or_else(|| panic!("{title}"));
        assert_eq!(loudness.source, LoudnessSource::ITunNorm, "{title}");
        close(loudness.lufs, ITUNNORM_LUFS);
        close(
            loudness.true_peak_dbtp.unwrap(),
            20.0 * (30_970.0f64 / 32_768.0).log10(),
        );
        assert_eq!(loudness.album_lufs, None);
    }
    assert_eq!(track_titled(&library, "cd rip").codec, "AIFF");
    assert_eq!(
        track_titled(&library, "Something").codec,
        "ALAC",
        "reading the atom did not disturb the rest"
    );
}

#[test]
fn the_best_source_wins_when_a_file_has_several() {
    let dir = copy_fixtures();
    let mp3 = dir.path().join("halo.mp3");
    add_itunnorm_to_mp3(&mp3, ITUNNORM_SAMPLE);
    retag(&mp3, |tag| {
        replaygain(tag, "-3.00 dB", "0.9", "-4.00 dB", "0.95")
    });
    let ogg = dir.path().join("zombie.ogg");
    retag(&ogg, |tag| {
        tag.insert_text(ItemKey::R128TrackGain, "-256".into());
    });
    let m4a = dir.path().join("something.m4a");
    add_itunnorm_to_mp4(&m4a, ITUNNORM_SAMPLE);
    retag(&m4a, |tag| {
        tag.insert_text(ItemKey::ReplayGainAlbumGain, "-5.00 dB".into());
    });
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let halo = loudness_of(&library, "Halo").unwrap();
    assert_eq!(halo.source, LoudnessSource::ReplayGain);
    close(halo.lufs, -15.0);
    let zombie = loudness_of(&library, "Zombie").unwrap();
    assert_eq!(zombie.source, LoudnessSource::R128Tag);
    close(zombie.lufs, -22.0);
    // Sound Check data has no album figure, so a ReplayGain album gain fills that in.
    let something = loudness_of(&library, "Something").unwrap();
    assert_eq!(something.source, LoudnessSource::ITunNorm);
    close(something.lufs, ITUNNORM_LUFS);
    close(something.album_lufs.unwrap(), -13.0);
}

#[test]
fn untrustworthy_tag_values_are_ignored() {
    let dir = copy_fixtures();
    let mp3 = dir.path().join("halo.mp3");
    add_itunnorm_to_mp3(&mp3, ITUNNORM_SAMPLE);
    // An absurd ReplayGain gain is skipped in favour of the next source.
    retag(&mp3, |tag| {
        replaygain(tag, "99.00 dB", "0.9", "-4.00 dB", "0.95")
    });
    let flac = dir.path().join("come_together.flac");
    // A gain that is not a number; a peak that is not finite.
    retag(&flac, |tag| replaygain(tag, "NaN dB", "0.9", "1e9", "inf"));
    let ogg = dir.path().join("zombie.ogg");
    retag(&ogg, |tag| {
        replaygain(tag, "-6.00 dB", "NaN", "-41.00 dB", "-3")
    });
    let m4a = dir.path().join("something.m4a");
    add_itunnorm_to_mp4(&m4a, " 00000000 00000000 0 0 0 0 0 0 0 0");
    let wav = dir.path().join("untagged.wav");
    retag(&wav, |tag| {
        tag.insert_text(ItemKey::R128TrackGain, "99999".into());
    });
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    assert_eq!(
        loudness_of(&library, "Halo").unwrap().source,
        LoudnessSource::ITunNorm
    );
    assert_eq!(loudness_of(&library, "Come Together"), None);
    let zombie = loudness_of(&library, "Zombie").unwrap();
    close(zombie.lufs, -12.0);
    assert_eq!(
        (
            zombie.true_peak_dbtp,
            zombie.album_lufs,
            zombie.album_true_peak_dbtp
        ),
        (None, None, None)
    );
    assert_eq!(
        loudness_of(&library, "Something"),
        None,
        "an all-zero Sound Check tag records nothing"
    );
    assert_eq!(loudness_of(&library, "untagged"), None);
}

#[test]
fn analyzed_loudness_is_stored_and_checked() {
    let (mut library, _dir) = scanned_library();
    let id = track_titled(&library, "Halo").id;

    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: Some(vec![1, 2, 3]),
            },
        )
        .unwrap();

    let stored = library.track(id).unwrap().unwrap().loudness.unwrap();
    assert_eq!(stored.source, LoudnessSource::Analyzed);
    assert_eq!((stored.lufs, stored.true_peak_dbtp), (-14.2, Some(-0.8)));
    assert_eq!(library.loudness_state(id).unwrap(), Some(vec![1, 2, 3]));
    assert_eq!(
        library
            .loudness_state(track_titled(&library, "Zombie").id)
            .unwrap(),
        None
    );

    for bad in [
        Loudness {
            lufs: f64::NAN,
            true_peak_dbtp: 0.0,
            state: None,
        },
        Loudness {
            lufs: -14.0,
            true_peak_dbtp: f64::INFINITY,
            state: None,
        },
        Loudness {
            lufs: 50.0,
            true_peak_dbtp: 0.0,
            state: None,
        },
        Loudness {
            lufs: -200.0,
            true_peak_dbtp: 0.0,
            state: None,
        },
        Loudness {
            lufs: -14.0,
            true_peak_dbtp: 99.0,
            state: None,
        },
    ] {
        assert!(
            matches!(
                library.set_loudness(id, &bad),
                Err(unsub_library::Error::InvalidLoudness(_))
            ),
            "{bad:?}"
        );
    }
    assert!(matches!(
        library.set_loudness(
            9_999,
            &Loudness {
                lufs: -14.0,
                true_peak_dbtp: 0.0,
                state: None
            }
        ),
        Err(unsub_library::Error::TrackNotFound(9_999))
    ));
    assert_eq!(
        library.track(id).unwrap().unwrap().loudness.unwrap().lufs,
        -14.2,
        "bad values change nothing"
    );
}

#[test]
fn an_analysis_replaces_what_the_tags_said_but_keeps_the_albums_stated_loudness() {
    let dir = copy_fixtures();
    retag(&dir.path().join("halo.mp3"), |tag| {
        replaygain(tag, "-6.50 dB", "0.9", "-7.25 dB", "1.0")
    });
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;

    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -9.0,
                true_peak_dbtp: -0.3,
                state: None,
            },
        )
        .unwrap();

    let loudness = library.track(id).unwrap().unwrap().loudness.unwrap();
    assert_eq!(
        (loudness.source, loudness.lufs),
        (LoudnessSource::Analyzed, -9.0)
    );
    close(loudness.album_lufs.unwrap(), -10.75);
}

#[test]
fn tracks_needing_loudness_are_local_present_and_unmeasured() {
    let dir = copy_fixtures();
    retag(&dir.path().join("halo.mp3"), |tag| {
        replaygain(tag, "-6.50 dB", "0.9", "-7.25 dB", "1.0")
    });
    let cloud = dir.path().join("cloud.mp3");
    fs::write(&cloud, b"placeholder").unwrap();
    fs::write(cloud.with_extension("evicted"), b"").unwrap();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    scan(&mut library, dir.path());
    let zombie = track_titled(&library, "Zombie");
    fs::remove_file(&zombie.path).unwrap();
    library.check_missing().unwrap();

    let needing = library.tracks_needing_loudness(100).unwrap();

    let titles: Vec<String> = needing
        .iter()
        .map(|(id, path)| {
            assert_eq!(path, Path::new(&library.track(*id).unwrap().unwrap().path));
            library.track(*id).unwrap().unwrap().title
        })
        .collect();
    assert!(!titles.contains(&"Halo".to_owned()), "tag-measured");
    assert!(!titles.contains(&"Zombie".to_owned()), "missing");
    assert!(!titles.contains(&"cloud".to_owned()), "still in the cloud");
    assert!(
        titles.contains(&"Episode 12".to_owned()) && titles.contains(&"Chapter 1".to_owned()),
        "podcasts and audiobooks are measured too"
    );
    assert_eq!(needing.len(), 6);
    assert!(
        needing.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "oldest first"
    );

    assert_eq!(library.tracks_needing_loudness(2).unwrap(), needing[..2]);
    assert!(library.tracks_needing_loudness(0).unwrap().is_empty());
    let (first, _) = needing[0].clone();
    library
        .set_loudness(
            first,
            &Loudness {
                lufs: -14.0,
                true_peak_dbtp: 0.0,
                state: None,
            },
        )
        .unwrap();
    assert_eq!(library.tracks_needing_loudness(100).unwrap().len(), 5);
}

fn measured(id: i64, album: &str, lufs: Option<f64>, peak: Option<f64>, ms: u64) -> Track {
    Track {
        loudness: lufs.map(|lufs| TrackLoudness {
            lufs,
            true_peak_dbtp: peak,
            source: LoudnessSource::Analyzed,
            album_lufs: None,
            album_true_peak_dbtp: None,
        }),
        album_artist: "Band".into(),
        ..track(id, &format!("song {id}"), "Band", album, ms)
    }
}

#[test]
fn an_albums_loudness_combines_its_tracks_by_length() {
    let library = library_of(&[
        measured(1, "Whole", Some(-20.0), Some(-3.0), 100_000),
        measured(2, "Whole", Some(-14.0), Some(-1.0), 100_000),
        measured(3, "Whole", Some(-17.0), Some(-6.0), 100_000),
        measured(4, "Partial", Some(-20.0), Some(-3.0), 100_000),
        measured(5, "Partial", None, None, 100_000),
        measured(6, "No Peaks", Some(-18.0), None, 100_000),
        measured(7, "No Peaks", Some(-18.0), Some(-2.0), 100_000),
    ]);
    let key = |album: &str| format!("band\u{1f}{}", album.to_lowercase());

    let whole = library.album_gain(&key("Whole")).unwrap().unwrap();
    let energy = (10f64.powf(-2.0) + 10f64.powf(-1.4) + 10f64.powf(-1.7)) / 3.0;
    close(whole.lufs, 10.0 * energy.log10());
    assert_eq!(whole.true_peak_dbtp, Some(-1.0));
    assert_eq!(
        library.album_gain(&key("Partial")).unwrap(),
        None,
        "a track unmeasured, so no album figure"
    );
    let no_peaks = library.album_gain(&key("No Peaks")).unwrap().unwrap();
    close(no_peaks.lufs, -18.0);
    assert_eq!(
        no_peaks.true_peak_dbtp, None,
        "a peak is only claimed when every track has one"
    );
    assert_eq!(library.album_gain("nobody\u{1f}nothing").unwrap(), None);
}

#[test]
fn an_albums_stated_loudness_wins_over_combining() {
    let dir = copy_fixtures();
    for file in ["come_together.flac", "something.m4a"] {
        retag(&dir.path().join(file), |tag| {
            replaygain(tag, "-6.00 dB", "0.9", "-7.00 dB", "0.99")
        });
    }
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Come Together").id;
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -30.0,
                true_peak_dbtp: -9.0,
                state: None,
            },
        )
        .unwrap();
    let album = library
        .albums(&unsub_library::TrackQuery::default())
        .unwrap();
    let abbey_road = album.iter().find(|a| a.album == "Abbey Road").unwrap();

    let gain = library.album_gain(&abbey_road.key).unwrap().unwrap();

    close(gain.lufs, -11.0);
    close(gain.true_peak_dbtp.unwrap(), 20.0 * 0.99f64.log10());
}

#[test]
fn sound_check_gain_aims_a_track_or_its_album_at_the_target() {
    let library = library_of(&[
        measured(1, "Album", Some(-20.0), Some(-3.0), 100_000),
        measured(2, "Album", Some(-14.0), Some(-1.0), 100_000),
        measured(3, "Lonely", Some(-22.0), Some(-5.0), 100_000),
        measured(4, "Partial", Some(-12.0), Some(-2.0), 100_000),
        measured(5, "Partial", None, None, 100_000),
        measured(6, "Unmeasured", None, None, 100_000),
    ]);
    let gain = |id, mode, target| library.sound_check_gain(id, mode, target).unwrap();

    assert_eq!(
        gain(1, GainMode::Track, -16.0),
        Some(Gain {
            db: 4.0,
            peak_dbtp: Some(-3.0)
        })
    );
    assert_eq!(
        gain(2, GainMode::Track, -16.0),
        Some(Gain {
            db: -2.0,
            peak_dbtp: Some(-1.0)
        })
    );
    let album = gain(1, GainMode::Album, -16.0).unwrap();
    let combined = 10.0 * ((10f64.powf(-2.0) + 10f64.powf(-1.4)) / 2.0).log10();
    close(album.db, -16.0 - combined);
    assert_eq!(album.peak_dbtp, Some(-1.0), "the album's loudest peak");
    assert_eq!(
        gain(1, GainMode::Album, -16.0),
        gain(2, GainMode::Album, -16.0),
        "one gain for the whole album"
    );
    // An unmeasured sibling means no album figure, so the track's own is used.
    assert_eq!(
        gain(4, GainMode::Album, -16.0),
        Some(Gain {
            db: -4.0,
            peak_dbtp: Some(-2.0)
        })
    );
    assert_eq!(
        gain(3, GainMode::Album, -23.0),
        gain(3, GainMode::Track, -23.0)
    );
    assert_eq!(gain(6, GainMode::Track, -16.0), None);
    assert_eq!(gain(6, GainMode::Album, -16.0), None);
    assert_eq!(gain(5, GainMode::Track, -16.0), None);
    assert_eq!(gain(9_999, GainMode::Track, -16.0), None);
    assert_eq!(gain(1, GainMode::Track, f64::NAN), None);
}

#[test]
fn a_get_info_edit_never_clears_a_measurement() {
    let (mut library, dir) = scanned_library();
    let id = track_titled(&library, "Halo").id;
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: Some(vec![9, 9]),
            },
        )
        .unwrap();
    let path = dir.path().join("halo.mp3");
    let before = fs::metadata(&path).unwrap().len();

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                title: Some("Edited".into()),
                album: Some("Elsewhere".into()),
                artwork: Some(Some(png())),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written, [id]);
    assert_ne!(
        fs::metadata(&path).unwrap().len(),
        before,
        "the file really was rewritten"
    );
    let row = library.track(id).unwrap().unwrap();
    assert_eq!(row.title, "Edited");
    let loudness = row.loudness.unwrap();
    assert_eq!(
        (loudness.source, loudness.lufs, loudness.true_peak_dbtp),
        (LoudnessSource::Analyzed, -14.2, Some(-0.8))
    );
    assert_eq!(library.loudness_state(id).unwrap(), Some(vec![9, 9]));

    // A library-only edit leaves it too.
    library
        .edit_tracks(
            &[id],
            &TrackEdits {
                rating: Some(40),
                kind: Some(MediaKind::Audiobook),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert_eq!(
        library.track(id).unwrap().unwrap().loudness.unwrap().lufs,
        -14.2
    );
}

fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

#[test]
fn a_re_read_of_an_unchanged_file_keeps_the_measurement_but_refreshes_tag_values() {
    let dir = copy_fixtures();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    retag(&dir.path().join("zombie.ogg"), |tag| {
        replaygain(tag, "-6.00 dB", "0.9", "-7.00 dB", "1.0")
    });
    let mut library = Library::open(&db_path).unwrap();
    scan(&mut library, dir.path());
    let measured_id = track_titled(&library, "Halo").id;
    let tagged_id = track_titled(&library, "Zombie").id;
    library
        .set_loudness(
            measured_id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: Some(vec![7]),
            },
        )
        .unwrap();
    // Pretend the tag-derived value is stale, then force every file to be read again.
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE tracks SET loudness_lufs = -50.0 WHERE id = ?1",
        [tagged_id],
    )
    .unwrap();
    conn.execute("UPDATE tracks SET parser_version = 1", [])
        .unwrap();

    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(summary.updated, 8);
    let measured = library
        .track(measured_id)
        .unwrap()
        .unwrap()
        .loudness
        .unwrap();
    assert_eq!(
        (measured.source, measured.lufs),
        (LoudnessSource::Analyzed, -14.2)
    );
    assert_eq!(library.loudness_state(measured_id).unwrap(), Some(vec![7]));
    close(
        library
            .track(tagged_id)
            .unwrap()
            .unwrap()
            .loudness
            .unwrap()
            .lufs,
        -12.0,
    );
}

#[test]
fn a_file_that_changed_loses_the_measurement_and_is_read_afresh() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: Some(vec![7]),
            },
        )
        .unwrap();

    // Another program edits the file: its size and time change, so the audio may have too.
    retag(&dir.path().join("halo.mp3"), |tag| {
        replaygain(tag, "-5.00 dB", "0.8", "-5.50 dB", "0.9")
    });
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    let loudness = library.track(id).unwrap().unwrap().loudness.unwrap();
    assert_eq!(loudness.source, LoudnessSource::ReplayGain);
    close(loudness.lufs, -13.0);
    assert_eq!(
        library.loudness_state(id).unwrap(),
        None,
        "the analyzer's state went with it"
    );

    // With nothing in the tags to fall back on, it is simply unmeasured again.
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: None,
            },
        )
        .unwrap();
    retag(&dir.path().join("halo.mp3"), |tag| {
        tag.remove_key(ItemKey::ReplayGainTrackGain);
        tag.remove_key(ItemKey::ReplayGainAlbumGain);
    });
    fs::File::options()
        .write(true)
        .open(dir.path().join("halo.mp3"))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(300))
        .unwrap();
    library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    assert!(library.track(id).unwrap().unwrap().loudness.is_none());
    assert!(
        library
            .tracks_needing_loudness(100)
            .unwrap()
            .iter()
            .any(|(i, _)| *i == id)
    );
}

#[test]
fn adding_a_known_file_again_keeps_its_measurement() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: None,
            },
        )
        .unwrap();

    let ids = added_ids(
        &mut library,
        &[(
            dir.path().join("halo.mp3"),
            TrackOverrides {
                genre: Some("Mixed".into()),
                ..TrackOverrides::default()
            },
        )],
    );

    assert_eq!(ids, [id]);
    assert_eq!(
        library.track(id).unwrap().unwrap().loudness.unwrap().lufs,
        -14.2
    );
}

#[test]
fn a_placeholder_keeps_its_measurement_through_eviction_and_download() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    scan(&mut library, dir.path());
    let id = track_titled(&library, "Halo").id;
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -14.2,
                true_peak_dbtp: -0.8,
                state: None,
            },
        )
        .unwrap();
    let marker = dir.path().join("halo.evicted");

    fs::write(&marker, b"").unwrap();
    assert_eq!(library.refresh_cloud_status(&[id]).unwrap(), [id]);
    assert!(
        library
            .tracks_needing_loudness(100)
            .unwrap()
            .iter()
            .all(|(i, _)| *i != id)
    );
    fs::remove_file(&marker).unwrap();
    assert_eq!(library.refresh_cloud_status(&[id]).unwrap(), [id]);

    assert_eq!(
        library.track(id).unwrap().unwrap().loudness.unwrap().lufs,
        -14.2
    );
}

#[test]
fn imported_rows_carry_their_loudness_and_a_reimport_replaces_it() {
    let mut library = Library::open_in_memory().unwrap();
    let loudness = TrackLoudness {
        lufs: -15.5,
        true_peak_dbtp: Some(-2.0),
        source: LoudnessSource::ITunNorm,
        album_lufs: Some(-16.0),
        album_true_peak_dbtp: None,
    };
    library
        .import_tracks(&[Track {
            loudness: Some(loudness.clone()),
            ..track(1, "a", "b", "c", 1)
        }])
        .unwrap();
    library
        .set_loudness(
            1,
            &Loudness {
                lufs: -10.0,
                true_peak_dbtp: -1.0,
                state: Some(vec![1]),
            },
        )
        .unwrap();
    assert_eq!(
        library.track(1).unwrap().unwrap().loudness.unwrap().source,
        LoudnessSource::Analyzed
    );

    library
        .import_tracks(&[Track {
            loudness: Some(loudness.clone()),
            ..track(1, "a", "b", "c", 1)
        }])
        .unwrap();
    assert_eq!(library.track(1).unwrap().unwrap().loudness, Some(loudness));
    assert_eq!(library.loudness_state(1).unwrap(), None);

    library
        .import_tracks(&[track(1, "a", "b", "c", 1)])
        .unwrap();
    assert_eq!(library.track(1).unwrap().unwrap().loudness, None);
}

#[test]
fn a_library_from_before_loudness_picks_it_up_from_the_tags_on_the_next_scan() {
    let dir = copy_fixtures();
    let db = TempDir::new().unwrap();
    let db_path = db.path().join("library.db");
    retag(&dir.path().join("halo.mp3"), |tag| {
        replaygain(tag, "-6.50 dB", "0.9", "-7.25 dB", "1.0")
    });
    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, dir.path());
    }
    rewind_schema(&db_path, 6);
    // What the previous build left behind: rows read by its tag parser.
    Connection::open(&db_path)
        .unwrap()
        .execute("UPDATE tracks SET parser_version = 2", [])
        .unwrap();

    let mut library = Library::open(&db_path).unwrap();
    assert!(
        library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .all(|t| t.loudness.is_none())
    );
    let summary = library
        .add_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();

    assert_eq!(
        summary.updated, 8,
        "the newer parser reads every file once more"
    );
    close(loudness_of(&library, "Halo").unwrap().lufs, -11.5);
    assert_eq!(library.tracks_needing_loudness(100).unwrap().len(), 7);
}

#[test]
fn loudness_json_and_the_ipod_value_follow_their_contracts() {
    let (mut library, _dir) = scanned_library();
    let id = track_titled(&library, "Halo").id;
    library
        .set_loudness(
            id,
            &Loudness {
                lufs: -20.0,
                true_peak_dbtp: -3.0,
                state: None,
            },
        )
        .unwrap();

    let json = serde_json::to_value(library.track(id).unwrap().unwrap()).unwrap();
    assert_eq!(json["loudness"]["source"], "analyzed");
    assert_eq!(json["loudness"]["lufs"], -20.0);
    assert_eq!(json["loudness"]["truePeakDbtp"], -3.0);
    assert!(json["loudness"]["albumLufs"].is_null());
    assert!(serde_json::to_value(track_titled(&library, "Zombie")).unwrap()["loudness"].is_null());
    assert_eq!(
        serde_json::from_str::<GainMode>("\"album\"").unwrap(),
        GainMode::Album
    );
    let gain = library
        .sound_check_gain(id, GainMode::Track, -16.0)
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(gain).unwrap()["peakDbtp"], -3.0);

    // The 4 dB boost a -20 LUFS track needs for -16 LUFS, as the iPod stores it.
    assert_eq!(itunes_soundcheck(gain.db), 398);
}
