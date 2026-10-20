use std::fs;
use std::path::{Path, PathBuf};

use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::id3::v2::{
    BinaryFrame, Frame, FrameId, Id3v2Tag, SyncTextContentType, SynchronizedTextFrame,
    TextInformationFrame, TimestampFormat, UnsynchronizedTextFrame,
};
use lofty::mp4::{AtomIdent, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::VorbisFile;
use lofty::tag::{ItemKey, Tag, TagExt, TagType};
use rusqlite::Connection;
use tempfile::TempDir;
use unsub_library::{Error, Library, Lyrics, LyricsSource, TrackEdits, TrackOverrides};

use crate::common::{
    added_ids, all_kinds, copy_fixtures, fixtures_dir, rewind_schema, scan, track,
};

const WORDS: &str = "First line\nSecond line";

fn copy(fixture: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(Path::new(fixture).file_name().unwrap());
    fs::copy(fixtures_dir().join(fixture), &path).unwrap();
    (dir, path)
}

fn added(path: &Path) -> (Library, i64) {
    let mut library = Library::open_in_memory().unwrap();
    let id = added_ids(
        &mut library,
        &[(path.to_path_buf(), TrackOverrides::default())],
    )[0];
    (library, id)
}

fn lyrics_of(path: &Path) -> Option<Lyrics> {
    let (library, id) = added(path);
    library.lyrics(id).unwrap()
}

fn has_lyrics(library: &Library, id: i64) -> bool {
    library.track(id).unwrap().unwrap().has_lyrics
}

fn plain(text: &str) -> Lyrics {
    Lyrics {
        text: text.to_owned(),
        synced: None,
        source: LyricsSource::Tag,
    }
}

fn timed(lines: &[(u64, &str)]) -> Vec<(u64, String)> {
    lines
        .iter()
        .map(|(ms, line)| (*ms, (*line).to_owned()))
        .collect()
}

fn write_tag(path: &Path, change: impl FnOnce(&mut Tag)) {
    let mut tagged = lofty::read_from_path(path).unwrap();
    change(tagged.primary_tag_mut().expect("the fixture is tagged"));
    tagged.save_to_path(path, WriteOptions::default()).unwrap();
}

/// Adds frames to the ID3v2 tag an MP3 already has, keeping what is there.
fn write_id3(path: &Path, frames: Vec<Frame<'static>>) {
    let mut file =
        MpegFile::read_from(&mut fs::File::open(path).unwrap(), ParseOptions::new()).unwrap();
    let tag = file.id3v2_mut().expect("the fixture has an ID3v2 tag");
    for frame in frames {
        tag.insert(frame);
    }
    file.save_to_path(path, WriteOptions::default()).unwrap();
}

fn uslt_in(encoding: TextEncoding, language: &[u8; 3], text: &str) -> Frame<'static> {
    Frame::UnsynchronizedText(UnsynchronizedTextFrame::new(
        encoding,
        *language,
        "",
        text.to_owned(),
    ))
}

fn uslt(language: &[u8; 3], text: &str) -> Frame<'static> {
    uslt_in(TextEncoding::UTF8, language, text)
}

fn tlan(code: &str) -> Frame<'static> {
    Frame::Text(TextInformationFrame::new(
        FrameId::new("TLAN").unwrap(),
        TextEncoding::UTF8,
        code.to_owned(),
    ))
}

fn sylt_in(format: TimestampFormat, lines: &[(u32, &str)]) -> Frame<'static> {
    let frame = SynchronizedTextFrame::new(
        TextEncoding::UTF8,
        *b"eng",
        format,
        SyncTextContentType::Lyrics,
        None,
        lines
            .iter()
            .map(|(ms, line)| (*ms, (*line).to_owned()))
            .collect(),
    );
    Frame::Binary(BinaryFrame::new(
        FrameId::new("SYLT").unwrap(),
        frame.as_bytes(WriteOptions::default()).unwrap(),
    ))
}

fn sylt(lines: &[(u32, &str)]) -> Frame<'static> {
    sylt_in(TimestampFormat::MS, lines)
}

fn id3_frames(path: &Path) -> Vec<Frame<'static>> {
    let file =
        MpegFile::read_from(&mut fs::File::open(path).unwrap(), ParseOptions::new()).unwrap();
    file.id3v2().unwrap().clone().into_iter().collect()
}

/// What lofty itself finds in the file, to confirm the library wrote real tags.
fn lofty_lyrics(path: &Path) -> Option<String> {
    lofty::read_from_path(path)
        .unwrap()
        .tags()
        .iter()
        .find_map(|tag| {
            [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
                .into_iter()
                .find_map(|key| tag.get_string(key))
        })
        .map(str::to_owned)
}

/// A fixture without lyrics, then the same file after `write` gave it some.
fn reads(fixture: &str, write: impl FnOnce(&Path)) -> Lyrics {
    let (_dir, path) = copy(fixture);
    let (library, id) = added(&path);
    assert_eq!(library.lyrics(id).unwrap(), None, "{fixture}");
    assert!(!has_lyrics(&library, id), "{fixture}");

    write(&path);

    let (library, id) = added(&path);
    assert!(has_lyrics(&library, id), "{fixture}");
    library.lyrics(id).unwrap().expect(fixture)
}

// ------------------------------------------------------------- containers

#[test]
fn mp3_lyrics_come_from_uslt() {
    let found = reads("halo.mp3", |path| {
        write_id3(path, vec![uslt(b"eng", WORDS)]);
    });

    assert_eq!(found, plain(WORDS));
}

#[test]
fn aiff_lyrics_come_from_its_id3_chunk() {
    let found = reads("lyrics/tone.aiff", |path| {
        let mut tag = Id3v2Tag::new();
        tag.insert(uslt(b"eng", WORDS));
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    });

    assert_eq!(found, plain(WORDS));
}

#[test]
fn mp4_lyrics_come_from_the_lyr_atom() {
    let found = reads("something.m4a", |path| {
        write_tag(path, |tag| {
            tag.insert_text(ItemKey::Lyrics, WORDS.to_owned());
        });
    });

    assert_eq!(found, plain(WORDS));
}

#[test]
fn vorbis_flac_and_opus_lyrics_come_from_either_comment() {
    for (fixture, key) in [
        ("zombie.ogg", ItemKey::Lyrics),
        ("come_together.flac", ItemKey::UnsyncLyrics),
        ("lyrics/tone.opus", ItemKey::Lyrics),
        ("lyrics/tone.opus", ItemKey::UnsyncLyrics),
    ] {
        let found = reads(fixture, |path| {
            write_tag(path, |tag| {
                tag.insert_text(key, WORDS.to_owned());
            });
        });

        assert_eq!(found, plain(WORDS), "{fixture} {key:?}");
    }
}

#[test]
fn vorbis_field_names_are_matched_in_any_case_and_lyrics_beats_unsyncedlyrics() {
    let (_dir, path) = copy("zombie.ogg");
    let mut file =
        VorbisFile::read_from(&mut fs::File::open(&path).unwrap(), ParseOptions::new()).unwrap();
    file.vorbis_comments_mut()
        .push("UNSYNCEDLYRICS".to_owned(), "Unsynced".to_owned());
    file.vorbis_comments_mut()
        .push("lyrics".to_owned(), "Lowercase".to_owned());
    file.save_to_path(&path, WriteOptions::default()).unwrap();

    assert_eq!(lyrics_of(&path), Some(plain("Lowercase")));
}

#[test]
fn ape_lyrics_come_from_the_lyrics_item() {
    let found = reads("halo.mp3", |path| {
        let mut tag = Tag::new(TagType::Ape);
        tag.insert_text(ItemKey::Lyrics, WORDS.to_owned());
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    });

    assert_eq!(found, plain(WORDS));
}

#[test]
fn id3_lyrics_are_chosen_by_the_files_own_language_else_the_first() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(
        &path,
        vec![
            uslt(b"fra", "En fran\u{e7}ais"),
            uslt(b"eng", "In English"),
            uslt(b"deu", ""),
        ],
    );
    assert_eq!(
        lyrics_of(&path).unwrap().text,
        "En fran\u{e7}ais",
        "no language declared: the first"
    );

    write_id3(&path, vec![tlan("eng")]);
    assert_eq!(lyrics_of(&path).unwrap().text, "In English");

    write_id3(&path, vec![tlan("ita")]);
    assert_eq!(
        lyrics_of(&path).unwrap().text,
        "En fran\u{e7}ais",
        "a language none of them is in: the first again"
    );
}

#[test]
fn utf8_declared_as_latin1_is_repaired_like_other_tag_text() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(
        &path,
        vec![uslt_in(
            TextEncoding::Latin1,
            b"eng",
            "Garai J\u{c3}\u{b3}zsef",
        )],
    );

    assert_eq!(lyrics_of(&path).unwrap().text, "Garai J\u{f3}zsef");
}

#[test]
fn timed_lyrics_come_from_sylt_and_count_for_has_lyrics() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(
        &path,
        vec![sylt(&[(2_000, "second"), (500, "first\n"), (4_000, "")])],
    );

    let (library, id) = added(&path);

    assert!(has_lyrics(&library, id));
    assert_eq!(
        library.lyrics(id).unwrap(),
        Some(Lyrics {
            text: "first\nsecond".into(),
            synced: Some(timed(&[(500, "first"), (2_000, "second"), (4_000, "")])),
            source: LyricsSource::Tag,
        })
    );
}

#[test]
fn timed_frames_that_cannot_be_read_as_lyrics_are_not_lyrics() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(
        &path,
        vec![sylt_in(TimestampFormat::MPEG, &[(10, "frames")])],
    );

    let (library, id) = added(&path);

    assert!(!has_lyrics(&library, id));
    assert_eq!(library.lyrics(id).unwrap(), None);
}

// --------------------------------------------------------------- sidecars

#[test]
fn a_sidecar_supplies_the_timed_lines_whatever_the_tags_hold() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(
        &path,
        vec![uslt(b"eng", "Tag words"), sylt(&[(1_000, "tag line")])],
    );
    fs::write(
        path.with_extension("lrc"),
        "\u{feff}[ar:Someone]\r\n[offset:+500]\r\n[00:02.00]Sidecar one\r\n[00:05.50]Sidecar two\r\n",
    )
    .unwrap();

    assert_eq!(
        lyrics_of(&path),
        Some(Lyrics {
            text: "Tag words".into(),
            synced: Some(timed(&[(1_500, "Sidecar one"), (5_000, "Sidecar two")])),
            source: LyricsSource::Tag,
        })
    );
}

#[test]
fn a_sidecar_alone_gives_text_and_timed_lines_but_not_has_lyrics() {
    let (_dir, path) = copy("halo.mp3");
    fs::write(
        path.with_extension("LRC"),
        "[00:01.00][00:03.00]Repeated\n[00:02.00]Between\n",
    )
    .unwrap();

    let (library, id) = added(&path);

    assert!(
        !has_lyrics(&library, id),
        "a sidecar can appear without the audio changing, so scans do not look"
    );
    assert_eq!(
        library.lyrics(id).unwrap(),
        Some(Lyrics {
            text: "Repeated\nBetween\nRepeated".into(),
            synced: Some(timed(&[
                (1_000, "Repeated"),
                (2_000, "Between"),
                (3_000, "Repeated")
            ])),
            source: LyricsSource::Sidecar,
        })
    );
}

#[test]
fn a_sidecar_with_a_name_of_its_own_is_not_picked_up() {
    let (_dir, path) = copy("halo.mp3");
    fs::write(path.with_file_name("other.lrc"), "[00:01.00]Not mine").unwrap();
    fs::write(path.with_extension("lrc.txt"), "[00:01.00]Not mine").unwrap();

    assert_eq!(lyrics_of(&path), None);
}

#[test]
fn a_malformed_sidecar_costs_only_its_damaged_lines_or_is_ignored() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(&path, vec![sylt(&[(1_000, "from the tag")])]);

    for ruined in [
        "just words\nno timing at all",
        "[00:99.00]bad\n[00:xx]worse\n[00:06.00 unclosed",
        "[ar:A]\n[ti:T]\n[00:01.00]\n",
        "",
    ] {
        fs::write(path.with_extension("lrc"), ruined).unwrap();
        assert_eq!(
            lyrics_of(&path).unwrap().synced,
            Some(timed(&[(1_000, "from the tag")])),
            "{ruined:?}: the tag's timing stands"
        );
    }

    fs::write(
        path.with_extension("lrc"),
        "[00:99.00]bad\n[00:02.00]good\n[00:03.00 unclosed\n[x:y]meta\n[00:04.00]fine\n",
    )
    .unwrap();
    assert_eq!(
        lyrics_of(&path).unwrap().synced,
        Some(timed(&[(2_000, "good"), (4_000, "fine")]))
    );

    // Bytes that are not text at all neither fail nor invent lines.
    let mut binary: Vec<u8> = (0..=255).collect();
    binary.extend(b"\n[00:01.00]after the noise\n");
    fs::write(path.with_extension("lrc"), binary).unwrap();
    assert!(lyrics_of(&path).unwrap().synced.is_some());

    fs::remove_file(path.with_extension("lrc")).unwrap();
    fs::create_dir(path.with_extension("lrc")).unwrap();
    assert_eq!(
        lyrics_of(&path).unwrap().synced,
        Some(timed(&[(1_000, "from the tag")])),
        "a directory with that name is no sidecar"
    );
}

// ----------------------------------------------------- untrusted contents

#[test]
fn lyrics_are_cleaned_and_cut_at_256_kb() {
    let (_dir, path) = copy("zombie.ogg");
    let long = "word ".repeat(100_000);
    write_tag(&path, |tag| {
        tag.insert_text(ItemKey::Lyrics, long);
    });

    let text = lyrics_of(&path).unwrap().text;

    assert!(text.len() <= 256 * 1024, "{}", text.len());
    assert!(text.len() > 250 * 1024, "most of it is kept");

    write_tag(&path, |tag| {
        tag.insert_text(
            ItemKey::Lyrics,
            "\u{feff}a\u{7}b\r\nc\rd\u{0}e\u{1b}[31mf\u{85}\tg\n\n\nh\r\n".to_owned(),
        );
    });
    assert_eq!(lyrics_of(&path).unwrap().text, "ab\nc\nde[31mf g\n\n\nh");
}

#[test]
fn a_huge_sidecar_is_bounded_in_lines_and_bytes() {
    let (_dir, path) = copy("halo.mp3");
    let big: String = (0..200_000)
        .map(|n| format!("[{:02}:{:02}.00]line {n}\n", n / 60 % 100, n % 60))
        .collect();
    assert!(big.len() > 2_000_000);
    fs::write(path.with_extension("lrc"), &big).unwrap();

    let found = lyrics_of(&path).unwrap();

    let lines = found.synced.unwrap();
    assert!(lines.len() <= 10_000, "{}", lines.len());
    assert!(lines.len() > 1_000);
    assert!(lines.iter().map(|(_, line)| line.len()).sum::<usize>() <= 256 * 1024);
    assert!(found.text.len() <= 256 * 1024);
}

// -------------------------------------------------------------- cloud files

#[test]
fn placeholders_are_never_opened_not_the_audio_and_not_the_sidecar() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(&path, vec![uslt(b"eng", "In the file")]);
    let sidecar = path.with_extension("lrc");
    fs::write(&sidecar, "[00:01.00]In the sidecar").unwrap();
    let (mut library, id) = added(&path);
    // Each file is a placeholder on its own marker, as in iCloud Drive.
    library.set_dataless_check(|path| {
        let name = path.file_name().unwrap().to_string_lossy();
        path.with_file_name(format!("{name}.evicted")).exists()
    });
    let evict = |file: &Path| {
        let name = file.file_name().unwrap().to_string_lossy();
        fs::write(file.with_file_name(format!("{name}.evicted")), b"").unwrap();
    };
    assert_eq!(library.lyrics(id).unwrap().unwrap().text, "In the file");

    // The sidecar is valid, so only not opening it explains its lines being absent.
    evict(&sidecar);
    assert_eq!(
        library.lyrics(id).unwrap().unwrap(),
        plain("In the file"),
        "the sidecar is skipped, the audio still read"
    );

    let audio_contents = fs::read(&path).unwrap();
    fs::write(&path, b"not audio, and opening it would fail").unwrap();
    evict(&path);
    assert_eq!(library.lyrics(id).unwrap(), None);

    // A local sidecar beside a placeholder is still read.
    fs::remove_file(sidecar.with_file_name("halo.lrc.evicted")).unwrap();
    assert_eq!(
        library.lyrics(id).unwrap().unwrap().source,
        LyricsSource::Sidecar
    );

    fs::remove_file(path.with_file_name("halo.mp3.evicted")).unwrap();
    fs::write(&path, audio_contents).unwrap();
    assert_eq!(library.lyrics(id).unwrap().unwrap().text, "In the file");
}

#[test]
fn a_cloud_row_has_no_lyrics_until_downloaded_and_asking_is_safe() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("Artist/Album/01 Song.mp3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"a placeholder, not audio").unwrap();
    fs::write(path.with_extension("evicted"), b"").unwrap();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    scan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;
    assert!(library.track(id).unwrap().unwrap().in_cloud);

    assert_eq!(library.lyrics(id).unwrap(), None);
    assert!(!has_lyrics(&library, id));
    assert!(matches!(
        library.lyrics(9_999),
        Err(Error::TrackNotFound(9_999))
    ));
}

#[test]
fn a_file_gone_since_the_scan_is_an_error_not_a_panic() {
    let (_dir, path) = copy("halo.mp3");
    let (library, id) = added(&path);
    fs::remove_file(&path).unwrap();

    assert!(library.lyrics(id).is_err());
}

// ---------------------------------------------------------------- writing

#[test]
fn lyrics_are_written_into_every_container_and_cleared_again() {
    for fixture in [
        "halo.mp3",
        "come_together.flac",
        "zombie.ogg",
        "something.m4a",
        "lyrics/tone.opus",
        "lyrics/tone.aiff",
        "untagged.wav",
    ] {
        let (_dir, path) = copy(fixture);
        let (mut library, id) = added(&path);
        let write = |library: &mut Library, text: &str| {
            library
                .edit_tracks(
                    &[id],
                    &TrackEdits {
                        lyrics: Some(text.to_owned()),
                        ..TrackEdits::default()
                    },
                )
                .unwrap()
        };

        let report = write(&mut library, "Line one\r\nLine two\u{7}");

        assert_eq!(report.written, [id], "{fixture}");
        assert!(report.db_only.is_empty(), "{fixture}: {:?}", report.db_only);
        assert_eq!(
            library.lyrics(id).unwrap(),
            Some(plain("Line one\nLine two")),
            "{fixture}"
        );
        assert!(has_lyrics(&library, id), "{fixture}");
        assert_eq!(
            lofty_lyrics(&path).as_deref(),
            Some("Line one\nLine two"),
            "{fixture}: the file itself carries them"
        );

        let report = write(&mut library, "Replaced");
        assert_eq!(report.written, [id], "{fixture}");
        assert_eq!(
            lofty_lyrics(&path).as_deref(),
            Some("Replaced"),
            "{fixture}"
        );

        let report = write(&mut library, "");
        assert_eq!(report.written, [id], "{fixture}");
        assert_eq!(library.lyrics(id).unwrap(), None, "{fixture}");
        assert!(!has_lyrics(&library, id), "{fixture}");
        assert_eq!(lofty_lyrics(&path), None, "{fixture}: cleared in the file");
    }
}

#[test]
fn each_format_gets_its_own_field_and_stale_ones_go() {
    let edit = |library: &mut Library, id: i64, text: &str| {
        library
            .edit_tracks(
                &[id],
                &TrackEdits {
                    lyrics: Some(text.to_owned()),
                    ..TrackEdits::default()
                },
            )
            .unwrap();
    };

    let (_dir, mp3) = copy("halo.mp3");
    write_id3(&mp3, vec![uslt(b"fra", "Ancien"), uslt(b"eng", "Old")]);
    let (mut library, id) = added(&mp3);
    edit(&mut library, id, "Words");
    let uslt_texts: Vec<String> = id3_frames(&mp3)
        .into_iter()
        .filter_map(|f| match f {
            Frame::UnsynchronizedText(u) => Some(u.content.into_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(uslt_texts, ["Words"], "one USLT replaces them all");

    let (_dir, m4a) = copy("something.m4a");
    let (mut library, id) = added(&m4a);
    edit(&mut library, id, "Words");
    let file = Mp4File::read_from(&mut fs::File::open(&m4a).unwrap(), ParseOptions::new()).unwrap();
    assert!(
        file.ilst()
            .unwrap()
            .get(&AtomIdent::Fourcc(*b"\xa9lyr"))
            .is_some(),
        "the lyr atom"
    );

    let (_dir, ogg) = copy("zombie.ogg");
    let mut file =
        VorbisFile::read_from(&mut fs::File::open(&ogg).unwrap(), ParseOptions::new()).unwrap();
    file.vorbis_comments_mut()
        .push("UNSYNCEDLYRICS".to_owned(), "Stale".to_owned());
    file.vorbis_comments_mut()
        .push("LYRICS".to_owned(), "Stale".to_owned());
    file.save_to_path(&ogg, WriteOptions::default()).unwrap();
    let (mut library, id) = added(&ogg);
    let comments = |path: &Path| -> Vec<(String, String)> {
        let file =
            VorbisFile::read_from(&mut fs::File::open(path).unwrap(), ParseOptions::new()).unwrap();
        file.vorbis_comments()
            .items()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .filter(|(k, _)| k.to_ascii_uppercase().ends_with("LYRICS"))
            .collect()
    };
    edit(&mut library, id, "Fresh");
    assert_eq!(comments(&ogg), [("LYRICS".to_owned(), "Fresh".to_owned())]);
    assert_eq!(library.lyrics(id).unwrap(), Some(plain("Fresh")));
    edit(&mut library, id, "");
    assert!(comments(&ogg).is_empty(), "both fields are cleared");
}

#[test]
fn writing_lyrics_leaves_timed_lyrics_the_sidecar_and_other_tags_alone() {
    let (_dir, path) = copy("halo.mp3");
    write_id3(
        &path,
        vec![uslt(b"eng", "Old words"), sylt(&[(1_000, "timed")])],
    );
    let sidecar = path.with_extension("lrc");
    fs::write(&sidecar, "[00:03.00]from the sidecar").unwrap();
    let (mut library, id) = added(&path);
    let title = library.track(id).unwrap().unwrap().title;
    let timed_frame = |path: &Path| {
        id3_frames(path)
            .into_iter()
            .find_map(|f| match f {
                Frame::Binary(b) if b.id().as_str() == "SYLT" => Some(b.data.into_owned()),
                _ => None,
            })
            .expect("the SYLT frame")
    };
    let sylt_before = timed_frame(&path);

    library
        .edit_tracks(
            &[id],
            &TrackEdits {
                lyrics: Some("New words".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(timed_frame(&path), sylt_before);
    assert_eq!(
        fs::read_to_string(&sidecar).unwrap(),
        "[00:03.00]from the sidecar"
    );
    assert_eq!(library.track(id).unwrap().unwrap().title, title);
    assert_eq!(
        library.lyrics(id).unwrap(),
        Some(Lyrics {
            text: "New words".into(),
            synced: Some(timed(&[(3_000, "from the sidecar")])),
            source: LyricsSource::Tag,
        })
    );

    // Clearing the plain lyrics does not touch the timed ones either, so the
    // track still has lyrics: the words of the timed lines.
    library
        .edit_tracks(
            &[id],
            &TrackEdits {
                lyrics: Some(String::new()),
                ..TrackEdits::default()
            },
        )
        .unwrap();
    assert!(has_lyrics(&library, id));
    assert_eq!(timed_frame(&path), sylt_before);
    assert_eq!(
        library.lyrics(id).unwrap().unwrap().text,
        "from the sidecar"
    );
}

#[test]
fn editing_lyrics_together_with_other_fields_and_on_several_tracks() {
    let dir = copy_fixtures();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());
    let ids: Vec<i64> = library
        .tracks(&all_kinds())
        .unwrap()
        .iter()
        .filter(|t| t.path.ends_with(".mp3") || t.path.ends_with(".flac"))
        .map(|t| t.id)
        .collect();
    assert!(ids.len() >= 3);

    let report = library
        .edit_tracks(
            &ids,
            &TrackEdits {
                genre: Some("Spoken".into()),
                lyrics: Some("Shared words".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert_eq!(report.written.len(), ids.len());
    for id in ids {
        let row = library.track(id).unwrap().unwrap();
        assert_eq!(row.genre, "Spoken");
        assert!(row.has_lyrics);
        assert_eq!(
            library.lyrics(id).unwrap().unwrap().text,
            "Shared words",
            "{}",
            row.path
        );
    }
}

#[test]
fn lyrics_for_files_that_cannot_be_written_are_kept_in_the_library_and_pinned() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let music = dir.path().join("music");
    fs::create_dir(&music).unwrap();
    let path = music.join("zombie.ogg");
    fs::copy(fixtures_dir().join("zombie.ogg"), &path).unwrap();
    write_tag(&path, |tag| {
        tag.insert_text(ItemKey::Lyrics, "In the file".to_owned());
    });
    let mut library = Library::open(&db_path).unwrap();
    scan(&mut library, &music);
    let id = library.tracks(&all_kinds()).unwrap()[0].id;
    assert_eq!(library.lyrics(id).unwrap(), Some(plain("In the file")));

    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let untouched = fs::read(&path).unwrap();
    let edit = |library: &mut Library, text: &str| {
        library
            .edit_tracks(
                &[id],
                &TrackEdits {
                    lyrics: Some(text.to_owned()),
                    ..TrackEdits::default()
                },
            )
            .unwrap()
    };
    // A rescan that finds the file as it was changes nothing; a newer tag
    // parser re-reads everything, which is what must not undo the edit.
    let reread = |library: &mut Library| {
        Connection::open(&db_path)
            .unwrap()
            .execute("UPDATE tracks SET parser_version = 1", [])
            .unwrap();
        scan(library, &music);
    };

    let report = edit(&mut library, "Pinned words");

    assert!(report.written.is_empty());
    assert!(report.db_only[0].1.contains("read-only"));
    assert_eq!(fs::read(&path).unwrap(), untouched);
    let expected = Lyrics {
        text: "Pinned words".into(),
        synced: None,
        source: LyricsSource::Library,
    };
    assert_eq!(library.lyrics(id).unwrap(), Some(expected.clone()));
    assert!(has_lyrics(&library, id));
    reread(&mut library);
    assert_eq!(library.lyrics(id).unwrap(), Some(expected));
    assert!(has_lyrics(&library, id));

    // Cleared in the library, the file's own lyrics stay hidden.
    edit(&mut library, "");
    assert_eq!(library.lyrics(id).unwrap(), None);
    assert!(!has_lyrics(&library, id));
    reread(&mut library);
    assert_eq!(library.lyrics(id).unwrap(), None);
    assert!(!has_lyrics(&library, id));

    // Once the file can be written the lyrics go into it and the pin is released.
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    let report = edit(&mut library, "Written at last");
    assert_eq!(report.written, [id]);
    assert_eq!(
        library.lyrics(id).unwrap(),
        Some(plain("Written at last")),
        "from the file now, not the library"
    );
    reread(&mut library);
    assert_eq!(library.lyrics(id).unwrap(), Some(plain("Written at last")));
}

#[test]
fn lyrics_set_on_a_cloud_row_are_returned_without_opening_the_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("Artist/Album/01 Song.mp3");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"a placeholder, not audio").unwrap();
    fs::write(path.with_extension("evicted"), b"").unwrap();
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|path| path.with_extension("evicted").exists());
    scan(&mut library, dir.path());
    let id = library.tracks(&all_kinds()).unwrap()[0].id;

    let report = library
        .edit_tracks(
            &[id],
            &TrackEdits {
                lyrics: Some("Typed while offline".into()),
                ..TrackEdits::default()
            },
        )
        .unwrap();

    assert!(report.db_only[0].1.to_lowercase().contains("icloud"));
    assert!(has_lyrics(&library, id));
    assert_eq!(
        library.lyrics(id).unwrap(),
        Some(Lyrics {
            text: "Typed while offline".into(),
            synced: None,
            source: LyricsSource::Library,
        })
    );
}

#[test]
fn added_files_can_bring_lyrics_as_an_override() {
    let (_dir, path) = copy("halo.mp3");
    let mut library = Library::open_in_memory().unwrap();

    let id = added_ids(
        &mut library,
        &[(
            path,
            TrackOverrides {
                lyrics: Some("Brought along\r\nwith it".into()),
                ..TrackOverrides::default()
            },
        )],
    )[0];

    assert!(has_lyrics(&library, id));
    assert_eq!(
        library.lyrics(id).unwrap().unwrap(),
        Lyrics {
            text: "Brought along\nwith it".into(),
            synced: None,
            source: LyricsSource::Library,
        }
    );
}

// --------------------------------------------------------------- scanning

#[test]
fn scans_flag_embedded_lyrics_only() {
    let dir = copy_fixtures();
    write_id3(&dir.path().join("halo.mp3"), vec![uslt(b"eng", WORDS)]);
    write_id3(
        &dir.path().join("episode_12.mp3"),
        vec![sylt(&[(0, "timed only")])],
    );
    write_tag(&dir.path().join("zombie.ogg"), |tag| {
        tag.insert_text(ItemKey::UnsyncLyrics, WORDS.to_owned());
    });
    write_tag(&dir.path().join("something.m4a"), |tag| {
        tag.insert_text(ItemKey::Lyrics, "  \n ".to_owned());
    });
    fs::write(
        dir.path().join("bohemian_rhapsody.lrc"),
        "[00:01.00]Sidecar only",
    )
    .unwrap();
    let mut library = Library::open_in_memory().unwrap();
    scan(&mut library, dir.path());

    let flagged: Vec<String> = {
        let mut names: Vec<String> = library
            .tracks(&all_kinds())
            .unwrap()
            .iter()
            .filter(|t| t.has_lyrics)
            .map(|t| {
                Path::new(&t.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    };

    assert_eq!(flagged, ["episode_12.mp3", "halo.mp3", "zombie.ogg"]);
    let sidecar_only = library
        .tracks(&all_kinds())
        .unwrap()
        .into_iter()
        .find(|t| t.path.ends_with("bohemian_rhapsody.m4a"))
        .unwrap();
    assert!(library.lyrics(sidecar_only.id).unwrap().is_some());
}

#[test]
fn rows_scanned_before_lyrics_were_known_are_read_again_for_them() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("library.db");
    let music = dir.path().join("music");
    fs::create_dir(&music).unwrap();
    let path = music.join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    write_id3(&path, vec![uslt(b"eng", WORDS)]);
    let mut library = Library::open(&db_path).unwrap();
    scan(&mut library, &music);
    drop(library);
    rewind_schema(&db_path, 8);
    Connection::open(&db_path)
        .unwrap()
        .execute("UPDATE tracks SET parser_version = 3", [])
        .unwrap();

    let mut library = Library::open(&db_path).unwrap();
    let id = library.tracks(&all_kinds()).unwrap()[0].id;
    assert!(!has_lyrics(&library, id), "the new column starts empty");
    let summary = library.add_paths(&[music], |_| {}).unwrap();

    assert_eq!((summary.updated, summary.unchanged), (1, 0));
    assert!(has_lyrics(&library, id));
    assert_eq!(library.lyrics(id).unwrap(), Some(plain(WORDS)));
}

// ------------------------------------------------------------------ wire

#[test]
fn lyrics_and_edits_cross_the_wire_in_camel_case() {
    let edits: TrackEdits = serde_json::from_str(r#"{"lyrics":"Words","title":"T"}"#).unwrap();
    assert_eq!(edits.lyrics.as_deref(), Some("Words"));
    assert_eq!(
        serde_json::from_str::<TrackEdits>("{}").unwrap().lyrics,
        None
    );

    let json = serde_json::to_value(Lyrics {
        text: "a".into(),
        synced: Some(timed(&[(500, "a")])),
        source: LyricsSource::Sidecar,
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({"text": "a", "synced": [[500, "a"]], "source": "sidecar"})
    );
    for (source, name) in [
        (LyricsSource::Tag, "tag"),
        (LyricsSource::Library, "library"),
    ] {
        assert_eq!(serde_json::to_value(source).unwrap(), name);
    }

    let mut row = track(1, "T", "A", "B", 1_000);
    row.has_lyrics = true;
    assert_eq!(serde_json::to_value(&row).unwrap()["hasLyrics"], true);
}
