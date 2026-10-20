use std::fs;
use std::path::{Path, PathBuf};

use antistreaming_library::{Chapter, Error, Library, TrackOverrides};
use lofty::TextEncoding;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::id3::v2::{
    ChapterFrame, ChapterTableOfContentsFrame, CtocFlags, Frame, FrameId, FrameList,
    TextInformationFrame,
};
use lofty::mpeg::MpegFile;
use lofty::ogg::VorbisFile;
use tempfile::TempDir;

use crate::common::{all_kinds, fixtures_dir};

fn fixture(name: &str) -> PathBuf {
    fixtures_dir().join("chapters").join(name)
}

/// The file added to a fresh library, and its chapters.
fn chapters_of(path: &Path) -> Vec<Chapter> {
    let mut library = Library::open_in_memory().unwrap();
    let id = library
        .add_files(&[(path.to_path_buf(), TrackOverrides::default())])
        .unwrap()[0];
    library.chapters(id).unwrap()
}

fn summary(chapters: &[Chapter]) -> Vec<(&str, u64, Option<u64>)> {
    chapters
        .iter()
        .map(|c| (c.title.as_str(), c.start_ms, c.end_ms))
        .collect()
}

fn near(actual: Option<u64>, expected: u64) {
    let actual = actual.expect("an end time");
    assert!(actual.abs_diff(expected) <= 100, "{actual} vs {expected}");
}

fn copy(name: &str, dir: &TempDir) -> PathBuf {
    let path = dir.path().join(name);
    fs::copy(fixture(name), &path).unwrap();
    path
}

const FIXTURE_TITLES: [&str; 3] = ["Opening", "Middle: part 2", "The End"];

fn assert_fixture_chapters(chapters: &[Chapter]) {
    assert_eq!(
        chapters
            .iter()
            .map(|c| c.title.as_str())
            .collect::<Vec<_>>(),
        FIXTURE_TITLES
    );
    assert_eq!(
        chapters.iter().map(|c| c.start_ms).collect::<Vec<_>>(),
        [0, 2_000, 4_500]
    );
    assert_eq!(chapters[0].end_ms, Some(2_000));
    assert_eq!(chapters[1].end_ms, Some(4_500));
    near(chapters[2].end_ms, 6_000);
}

#[test]
fn chapters_are_read_from_every_container() {
    for name in ["book.m4b", "book.mp3", "book.ogg", "book.flac"] {
        let chapters = chapters_of(&fixture(name));
        assert_fixture_chapters(&chapters);
    }
}

/// Finds the first box of this type in the bytes and returns its start (size field) and length.
fn find_box(bytes: &[u8], kind: &[u8; 4], from: usize) -> (usize, usize) {
    let at = bytes[from..]
        .windows(4)
        .position(|w| w == kind)
        .expect("box present")
        + from;
    let size = u32::from_be_bytes(bytes[at - 4..at].try_into().unwrap()) as usize;
    (at - 4, size)
}

fn rename_box(bytes: &mut [u8], kind: &[u8; 4], new_kind: &[u8; 4], from: usize) {
    let (start, _) = find_box(bytes, kind, from);
    bytes[start + 4..start + 8].copy_from_slice(new_kind);
}

#[test]
fn quicktime_chapter_tracks_win_over_nero_lists() {
    let dir = TempDir::new().unwrap();
    let path = copy("book.m4b", &dir);
    let mut bytes = fs::read(&path).unwrap();
    // Make the Nero list say something else; a player that reads it would show these.
    let (start, size) = find_box(&bytes, b"chpl", 0);
    let chpl = &mut bytes[start..start + size];
    for (from, to) in [("Opening", "OPENING"), ("The End", "THE END")] {
        let at = chpl
            .windows(from.len())
            .position(|w| w == from.as_bytes())
            .unwrap();
        chpl[at..at + from.len()].copy_from_slice(to.as_bytes());
    }
    fs::write(&path, &bytes).unwrap();

    let chapters = chapters_of(&path);

    assert_fixture_chapters(&chapters);
}

#[test]
fn nero_lists_are_used_when_there_is_no_chapter_track() {
    let dir = TempDir::new().unwrap();
    let path = copy("book.m4b", &dir);
    let mut bytes = fs::read(&path).unwrap();
    rename_box(&mut bytes, b"chap", b"free", 0);
    fs::write(&path, &bytes).unwrap();

    let chapters = chapters_of(&path);

    assert_fixture_chapters(&chapters);
}

#[test]
fn a_chapter_track_alone_is_enough() {
    let dir = TempDir::new().unwrap();
    let path = copy("book.m4b", &dir);
    let mut bytes = fs::read(&path).unwrap();
    rename_box(&mut bytes, b"chpl", b"free", 0);
    fs::write(&path, &bytes).unwrap();

    assert_fixture_chapters(&chapters_of(&path));
}

#[test]
fn an_unreadable_chapter_track_falls_back_to_the_nero_list() {
    let dir = TempDir::new().unwrap();
    let path = copy("book.m4b", &dir);
    let mut bytes = fs::read(&path).unwrap();
    // Point the text track's first chunk far beyond the end of the file.
    let (first_stco, _) = find_box(&bytes, b"stco", 0);
    let (text_stco, _) = find_box(&bytes, b"stco", first_stco + 8);
    bytes[text_stco + 16..text_stco + 20].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
    fs::write(&path, &bytes).unwrap();

    let chapters = chapters_of(&path);

    assert_eq!(chapters.len(), 3, "from the Nero list");
    assert_eq!(chapters[1].title, "Middle: part 2");
}

#[test]
fn a_nero_chapter_past_the_end_of_the_audio_is_dropped() {
    let dir = TempDir::new().unwrap();
    let path = copy("book.m4b", &dir);
    let mut bytes = fs::read(&path).unwrap();
    rename_box(&mut bytes, b"chap", b"free", 0);
    // The last entry's start becomes ten minutes into a six-second file.
    let (start, size) = find_box(&bytes, b"chpl", 0);
    let chpl = &mut bytes[start..start + size];
    let at = chpl.windows(7).position(|w| w == b"The End").unwrap();
    let start_field = at - 1 - 8;
    chpl[start_field..start_field + 8].copy_from_slice(&6_000_000_000u64.to_be_bytes());
    fs::write(&path, &bytes).unwrap();

    let chapters = chapters_of(&path);

    assert_eq!(
        chapters
            .iter()
            .map(|c| c.title.as_str())
            .collect::<Vec<_>>(),
        ["Opening", "Middle: part 2"]
    );
    near(chapters[1].end_ms, 6_000);
}

#[test]
fn damaged_mp4s_give_what_survives_not_errors() {
    let dir = TempDir::new().unwrap();
    let whole = fs::read(fixture("book.m4b")).unwrap();
    let (moov, moov_size) = find_box(&whole, b"moov", 0);

    for (name, bytes) in [
        (
            "cut in the moov.m4b",
            whole[..moov + moov_size / 2].to_vec(),
        ),
        ("cut before the moov.m4b", whole[..moov - 5].to_vec()),
        (
            "ends with half a header.m4b",
            whole[..whole.len() - 3].to_vec(),
        ),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        let mut library = Library::open_in_memory().unwrap();
        // The scan may not accept such a file; the point is that asking for chapters is safe.
        if let Ok(ids) = library.add_files(&[(path.clone(), TrackOverrides::default())]) {
            let chapters = library.chapters(ids[0]).unwrap();
            assert!(chapters.len() <= 3, "{name}");
        }
    }

    // Garbage where the movie box should be.
    let mut garbled = whole.clone();
    garbled[moov + 8..moov + 100].fill(0xFF);
    let path = dir.path().join("garbled.m4b");
    fs::write(&path, garbled).unwrap();
    let mut library = Library::open_in_memory().unwrap();
    if let Ok(ids) = library.add_files(&[(path, TrackOverrides::default())]) {
        assert!(library.chapters(ids[0]).unwrap().len() <= 3);
    }
}

fn id3_book(dir: &TempDir, frames: Vec<Frame<'static>>) -> PathBuf {
    let path = dir.path().join("halo.mp3");
    fs::copy(fixtures_dir().join("halo.mp3"), &path).unwrap();
    let mut mpeg =
        MpegFile::read_from(&mut fs::File::open(&path).unwrap(), ParseOptions::new()).unwrap();
    for frame in frames {
        mpeg.id3v2_mut().unwrap().insert(frame);
    }
    mpeg.save_to_path(&path, WriteOptions::default()).unwrap();
    path
}

fn chap(id: &str, start: u32, end: u32, title: Option<&str>) -> Frame<'static> {
    let mut children = FrameList::new();
    if let Some(title) = title {
        children.insert(Frame::Text(TextInformationFrame::new(
            FrameId::new("TIT2").unwrap(),
            TextEncoding::UTF8,
            title.to_owned(),
        )));
    }
    Frame::Chapter(ChapterFrame::new(
        id.to_owned(),
        start..end,
        u32::MAX..u32::MAX,
        children,
    ))
}

fn toc(id: &str, ordered: bool, entries: &[&str]) -> Frame<'static> {
    let entries: Vec<std::borrow::Cow<'static, str>> = entries
        .iter()
        .map(|e| std::borrow::Cow::Owned((*e).to_owned()))
        .collect();
    Frame::TableOfContents(ChapterTableOfContentsFrame::new(
        id.to_owned(),
        CtocFlags {
            top_level: true,
            ordered,
        },
        entries,
        FrameList::new(),
    ))
}

#[test]
fn id3_chapters_follow_the_table_of_contents_and_the_files_length() {
    let dir = TempDir::new().unwrap();
    // The fixture is half a second long: the last chapter starts after it ends.
    let path = id3_book(
        &dir,
        vec![
            chap("one", 0, 200, Some("One")),
            chap("two", 200, 350, None),
            chap("three", 350, 480, Some("Three")),
            chap("late", 5_000, 6_000, Some("Past the end")),
            toc("toc", true, &["three", "one", "two", "late"]),
        ],
    );

    let chapters = chapters_of(&path);

    assert_eq!(
        summary(&chapters),
        [
            ("Three", 350, Some(480)),
            ("One", 0, Some(200)),
            ("Chapter 3", 200, Some(350))
        ]
    );
}

#[test]
fn id3_chapters_without_a_table_of_contents_are_in_time_order() {
    let dir = TempDir::new().unwrap();
    let path = id3_book(
        &dir,
        vec![chap("b", 300, 0, Some("B")), chap("a", 100, 300, Some("A"))],
    );

    let chapters = chapters_of(&path);

    assert_eq!(summary(&chapters)[..1], [("A", 100, Some(300))]);
    assert_eq!(chapters[1].start_ms, 300);
    near(chapters[1].end_ms, 576); // the file's own length, as the last chapter's end
}

fn vorbis_book(dir: &TempDir, comments: &[(&str, String)]) -> PathBuf {
    let path = dir.path().join("zombie.ogg");
    fs::copy(fixtures_dir().join("zombie.ogg"), &path).unwrap();
    let mut file =
        VorbisFile::read_from(&mut fs::File::open(&path).unwrap(), ParseOptions::new()).unwrap();
    for (key, value) in comments {
        file.vorbis_comments_mut()
            .push((*key).to_owned(), value.clone());
    }
    file.save_to_path(&path, WriteOptions::default()).unwrap();
    path
}

#[test]
fn vorbis_chapter_comments_tolerate_sloppy_tagging() {
    let dir = TempDir::new().unwrap();
    let comments: Vec<(&str, String)> = [
        ("chapter10", "00:00:00.400"),
        ("CHAPTER10NAME", "Fourth"),
        ("CHAPTER2", "00:00:00.100"),
        ("Chapter2Name", "Second"),
        ("CHAPTER003", "garbage"),
        ("CHAPTER003NAME", "Never shown"),
        ("CHAPTER004NAME", "No time"),
        ("CHAPTER005", "00:00:00.250"),
        ("CHAPTER006", "00:99:00.000"),
        ("CHAPTERX", "00:00:00.300"),
        ("CHAPTER007", "00:10:00.000"),
    ]
    .iter()
    .map(|(k, v)| (*k, (*v).to_owned()))
    .collect();
    let path = vorbis_book(&dir, &comments);

    let chapters = chapters_of(&path);

    // Chapter 5 has no name; the one at ten minutes is past the end of the file.
    assert_eq!(
        summary(&chapters),
        [
            ("Second", 100, Some(250)),
            ("Chapter 2", 250, Some(400)),
            ("Fourth", 400, Some(500))
        ]
    );
}

#[test]
fn a_flood_of_chapters_is_capped() {
    let dir = TempDir::new().unwrap();
    let mut comments = Vec::new();
    for n in 1..=12_000u32 {
        comments.push((
            "CHAPTER".to_owned() + &format!("{n:05}"),
            format!("00:00:00.{:03}", n % 400),
        ));
    }
    let borrowed: Vec<(&str, String)> = comments
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    let path = vorbis_book(&dir, &borrowed);

    let chapters = chapters_of(&path);

    assert_eq!(chapters.len(), 10_000);
}

#[test]
fn very_long_titles_are_cut() {
    let dir = TempDir::new().unwrap();
    let path = vorbis_book(
        &dir,
        &[
            ("CHAPTER001", "00:00:00.000".to_owned()),
            ("CHAPTER001NAME", "T".repeat(5_000)),
        ],
    );

    let chapters = chapters_of(&path);

    assert_eq!(chapters[0].title.chars().count(), 500);
}

#[test]
fn files_without_chapters_in_any_format_have_none() {
    let dir = TempDir::new().unwrap();
    let mut library = Library::open_in_memory().unwrap();
    let files: Vec<_> = [
        "come_together.flac",
        "halo.mp3",
        "something.m4a",
        "zombie.ogg",
        "untagged.wav",
    ]
    .iter()
    .map(|name| {
        let path = dir.path().join(name);
        fs::copy(fixtures_dir().join(name), &path).unwrap();
        (path, TrackOverrides::default())
    })
    .collect();
    let ids = library.add_files(&files).unwrap();

    for id in ids {
        assert!(library.chapters(id).unwrap().is_empty(), "{id}");
    }
}

#[test]
fn asking_for_chapters_is_safe_for_missing_tracks_files_and_placeholders() {
    let dir = TempDir::new().unwrap();
    let path = copy("book.m4b", &dir);
    let mut library = Library::open_in_memory().unwrap();
    library.set_dataless_check(|p| p.with_extension("evicted").exists());
    let id = library
        .add_files(&[(path.clone(), TrackOverrides::default())])
        .unwrap()[0];
    assert!(matches!(
        library.chapters(9_999),
        Err(Error::TrackNotFound(9_999))
    ));

    // A placeholder is not opened (that would download it), whatever is inside.
    fs::write(path.with_extension("evicted"), b"").unwrap();
    assert!(library.chapters(id).unwrap().is_empty());
    fs::remove_file(path.with_extension("evicted")).unwrap();
    assert_eq!(library.chapters(id).unwrap().len(), 3);

    // The file has gone since the scan.
    fs::remove_file(&path).unwrap();
    assert!(library.chapters(id).is_err());
    assert_eq!(library.tracks(&all_kinds()).unwrap().len(), 1);
}

#[test]
fn chapters_serialize_with_camel_case_names() {
    let chapters = chapters_of(&fixture("book.mp3"));

    let json = serde_json::to_value(&chapters[0]).unwrap();

    assert_eq!(json["title"], "Opening");
    assert_eq!(json["startMs"], 0);
    assert_eq!(json["endMs"], 2_000);
}
