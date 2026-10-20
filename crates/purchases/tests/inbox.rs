use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use filetime::FileTime;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use unsub_purchases::{Candidate, Error, Inbox, Store};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Real tagged audio: The Beatles, "Abbey Road". Only its tags matter here.
const BEATLES_FLAC: &[u8] = include_bytes!("fixtures/come_together.flac");
const BEYONCE_MP3: &[u8] = include_bytes!("fixtures/halo.mp3");

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    downloads: PathBuf,
    library: PathBuf,
    inbox: Inbox,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let downloads = root.join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let library = root.join("Library");
        let inbox = Inbox::open(&root.join("ledger.db"), &library).unwrap();
        Self {
            _dir: dir,
            root,
            downloads,
            library,
            inbox,
        }
    }

    fn scan(&mut self) -> Vec<Candidate> {
        let downloads = self.downloads.clone();
        self.inbox.scan(&[downloads]).unwrap()
    }

    /// Writes a finished-looking zip into Downloads.
    fn zip(&self, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = self.downloads.join(name);
        write_zip(&path, entries);
        age(&path);
        path
    }

    /// Lists everything under the library folder, as relative paths.
    fn library_contents(&self) -> Vec<String> {
        if !self.library.exists() {
            return Vec::new();
        }
        let mut found: Vec<String> = WalkDir::new(&self.library)
            .min_depth(1)
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path().strip_prefix(&self.library).unwrap();
                path.to_string_lossy().replace('\\', "/")
            })
            .collect();
        found.sort();
        found
    }
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    write_zip_with(path, entries, CompressionMethod::Stored);
}

fn write_zip_with(path: &Path, entries: &[(&str, &[u8])], method: CompressionMethod) {
    let mut writer = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap();
}

/// Makes a file or folder tree look like it was finished an hour ago.
fn age(path: &Path) {
    let long_ago = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(3600));
    for entry in WalkDir::new(path) {
        filetime::set_file_mtime(entry.unwrap().path(), long_ago).unwrap();
    }
}

fn folder(path: &Path, files: &[(&str, &[u8])]) {
    for (name, data) in files {
        let file = path.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, data).unwrap();
    }
    age(path);
}

/// The Beatles fixture with a comment added, the way a store might stamp its downloads.
fn flac_commented(comment: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("track.flac");
    fs::write(&path, BEATLES_FLAC).unwrap();
    let mut file = lofty::read_from_path(&path).unwrap();
    let tag = file.primary_tag_mut().unwrap();
    tag.insert_text(ItemKey::Comment, comment.to_owned());
    tag.save_to_path(&path, WriteOptions::default()).unwrap();
    fs::read(path).unwrap()
}

const BANDCAMP: &str = "Radiohead - OK Computer";

fn bandcamp_entries() -> Vec<(String, Vec<u8>)> {
    vec![
        (format!("{BANDCAMP} - 01 Airbag.flac"), vec![1; 4000]),
        (
            format!("{BANDCAMP} - 02 Paranoid Android.flac"),
            vec![2; 6000],
        ),
        ("cover.jpg".to_owned(), vec![3; 500]),
    ]
}

fn bandcamp_zip(env: &Env) -> PathBuf {
    let entries = bandcamp_entries();
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    env.zip(&format!("{BANDCAMP}.zip"), &borrowed)
}

// ---------------------------------------------------------------------------------------------
// What gets offered
// ---------------------------------------------------------------------------------------------

#[test]
fn offers_a_bandcamp_zip() {
    let mut env = Env::new();
    let zip = bandcamp_zip(&env);

    let found = env.scan();

    assert_eq!(found.len(), 1);
    let candidate = &found[0];
    assert_eq!(candidate.path, zip);
    assert_eq!(candidate.store, Store::Bandcamp);
    assert_eq!(candidate.artist, "Radiohead");
    assert_eq!(candidate.album, "OK Computer");
    assert_eq!(candidate.track_count, 2);
    assert_eq!(candidate.formats, ["FLAC"]);
    assert_eq!(candidate.bytes, fs::metadata(&zip).unwrap().len());
    assert_eq!(env.inbox.candidates().unwrap(), found);
}

#[test]
fn offers_a_qobuz_zip_wrapped_in_a_folder() {
    let mut env = Env::new();
    let name = "Björk - Homogenic (1997) [FLAC] [24B-96kHz]";
    env.zip(
        &format!("{name}.zip"),
        &[
            (&format!("{name}/01 - Hunter.flac"), &[1; 100]),
            (&format!("{name}/02 - Joga.flac"), &[2; 100]),
            (&format!("{name}/Booklet.pdf"), &[3; 100]),
        ],
    );

    let found = env.scan();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].store, Store::Qobuz);
    assert_eq!(found[0].artist, "Björk");
    assert_eq!(found[0].album, "Homogenic");
    assert_eq!(found[0].track_count, 2);
}

#[test]
fn recognizes_seven_digital_by_a_name_inside_the_zip() {
    let mut env = Env::new();
    env.zip(
        "Geogaddi.zip",
        &[("7digital/01 Ready Lets Go.mp3", BEYONCE_MP3)],
    );

    let found = env.scan();

    assert_eq!(found[0].store, Store::SevenDigital);
    assert_eq!(found[0].formats, ["MP3"]);
}

#[test]
fn recognizes_hdtracks_by_a_stamp_in_the_tags() {
    let mut env = Env::new();
    env.zip(
        "download.zip",
        &[(
            "01 Come Together.flac",
            &flac_commented("Purchased from HDtracks.com"),
        )],
    );

    let found = env.scan();

    assert_eq!(found[0].store, Store::Hdtracks);
    // The names said nothing about who made it, so the tags did.
    assert_eq!(found[0].artist, "The Beatles");
    assert_eq!(found[0].album, "Abbey Road");
}

#[test]
fn reads_the_tags_when_the_names_say_nothing() {
    let mut env = Env::new();
    env.zip(
        "download (3).zip",
        &[("01 Come Together.flac", BEATLES_FLAC)],
    );

    let found = env.scan();

    assert_eq!(found[0].store, Store::Unknown);
    assert_eq!(found[0].artist, "The Beatles");
    assert_eq!(found[0].album, "Abbey Road");
}

#[test]
fn falls_back_to_the_zip_name_when_there_are_no_tags() {
    let mut env = Env::new();
    env.zip("Some Album.zip", &[("01 Song.flac", b"not really audio")]);

    let found = env.scan();

    assert_eq!(found[0].artist, "Unknown Artist");
    assert_eq!(found[0].album, "Some Album");
}

#[test]
fn offers_a_folder_that_appears_after_the_first_scan() {
    let mut env = Env::new();
    assert!(
        env.scan().is_empty(),
        "the first scan just takes note of the folder"
    );

    let album = env.downloads.join("New Download");
    folder(
        &album,
        &[
            ("01 Come Together.flac", BEATLES_FLAC),
            ("cover.jpg", b"jpg"),
        ],
    );
    let found = env.scan();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, album);
    assert_eq!(found[0].artist, "The Beatles");
    assert_eq!(found[0].album, "Abbey Road");
    assert_eq!(found[0].track_count, 1);
    assert_eq!(found[0].formats, ["FLAC"]);
    assert_eq!(found[0].bytes, BEATLES_FLAC.len() as u64 + 3);
}

#[test]
fn does_not_offer_folders_that_were_already_there() {
    let mut env = Env::new();
    folder(
        &env.downloads.join("Old Album"),
        &[("01 Song.flac", b"audio")],
    );

    assert!(env.scan().is_empty());
    assert!(env.scan().is_empty());
    assert!(env.inbox.candidates().unwrap().is_empty());
}

#[test]
fn does_not_offer_a_folder_without_audio() {
    let mut env = Env::new();
    env.scan();
    folder(
        &env.downloads.join("Taxes"),
        &[("2025.pdf", b"pdf"), ("notes.txt", b"txt")],
    );

    assert!(env.scan().is_empty());
}

#[test]
fn skips_the_folder_a_browser_unzipped_next_to_its_zip() {
    let mut env = Env::new();
    env.scan();
    bandcamp_zip(&env);
    folder(
        &env.downloads.join(BANDCAMP),
        &[(&format!("{BANDCAMP} - 01 Airbag.flac"), &[1; 4000])],
    );

    let found = env.scan();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, env.downloads.join(format!("{BANDCAMP}.zip")));
}

#[test]
fn ignores_zips_with_no_audio_and_hidden_files() {
    let mut env = Env::new();
    env.zip("taxes.zip", &[("2025.pdf", b"pdf")]);
    env.zip("macos.zip", &[("__MACOSX/._01.flac", b"junk")]);
    let hidden = env.downloads.join(".hidden.zip");
    write_zip(&hidden, &[("01.flac", b"audio")]);
    age(&hidden);

    assert!(env.scan().is_empty());
}

#[test]
fn a_missing_watch_directory_is_not_an_error() {
    let mut env = Env::new();
    let missing = env.root.join("no such folder");
    assert!(env.inbox.scan(&[missing]).unwrap().is_empty());
}

#[test]
fn lists_the_newest_candidate_first() {
    let mut env = Env::new();
    env.zip("First.zip", &[("01 Song.flac", b"one")]);
    env.scan();
    env.zip("Second.zip", &[("01 Song.flac", b"two")]);
    env.scan();

    let albums: Vec<_> = env
        .inbox
        .candidates()
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.album)
        .collect();

    assert_eq!(albums, ["Second", "First"]);
}

#[test]
fn scanning_again_does_not_report_the_same_candidate_twice() {
    let mut env = Env::new();
    bandcamp_zip(&env);

    assert_eq!(env.scan().len(), 1);
    assert!(env.scan().is_empty());
    assert_eq!(env.inbox.candidates().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Downloads still arriving
// ---------------------------------------------------------------------------------------------

#[test]
fn ignores_downloads_that_are_still_in_progress() {
    let mut env = Env::new();
    for name in [
        "Album.zip.crdownload",
        "Album.zip.part",
        "Album.zip.partial",
        "Album.flac.crdownload",
    ] {
        let path = env.downloads.join(name);
        write_zip(&path, &[("01.flac", b"audio")]);
        age(&path);
    }
    // Safari keeps an unfinished download in a folder named like this.
    folder(
        &env.downloads.join("Album.zip.download"),
        &[("Album.zip", b"half")],
    );
    env.scan();
    // A folder still holding a partial file is not done either.
    folder(
        &env.downloads.join("Half Album"),
        &[("01.flac", b"audio"), ("02.flac.crdownload", b"partial")],
    );

    assert!(env.scan().is_empty());
}

#[test]
fn waits_while_a_zip_was_written_to_in_the_last_few_seconds() {
    let mut env = Env::new();
    let zip = env.downloads.join("Fresh.zip");
    write_zip(&zip, &[("01 Song.flac", b"audio")]);

    assert!(env.scan().is_empty(), "just written");

    age(&zip);
    assert_eq!(env.scan().len(), 1, "settled");
}

#[test]
fn waits_while_a_folder_was_written_to_in_the_last_few_seconds() {
    let mut env = Env::new();
    env.scan();
    let album = env.downloads.join("Fresh Album");
    fs::create_dir(&album).unwrap();
    fs::write(album.join("01 Song.flac"), b"audio").unwrap();

    assert!(env.scan().is_empty(), "just written");

    age(&album);
    assert_eq!(env.scan().len(), 1, "settled");
}

#[test]
fn a_truncated_zip_is_retried_once_it_is_complete() {
    let mut env = Env::new();
    let zip = bandcamp_zip(&env);
    let complete = fs::read(&zip).unwrap();
    fs::write(&zip, &complete[..complete.len() / 2]).unwrap();
    age(&zip);

    assert!(env.scan().is_empty());

    fs::write(&zip, &complete).unwrap();
    age(&zip);
    assert_eq!(env.scan().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// The ledger
// ---------------------------------------------------------------------------------------------

#[test]
fn the_same_download_is_never_offered_twice() {
    let mut env = Env::new();
    let first = bandcamp_zip(&env);
    assert_eq!(env.scan().len(), 1);

    // Downloaded again under another name, and again after being ignored.
    let copy = env.downloads.join("Radiohead - OK Computer (1).zip");
    fs::copy(&first, &copy).unwrap();
    age(&copy);
    assert!(
        env.scan().is_empty(),
        "identical content while still pending"
    );

    let id = env.inbox.candidates().unwrap()[0].id;
    env.inbox.ignore(id).unwrap();
    let again = env.downloads.join("Radiohead - OK Computer (2).zip");
    fs::copy(&first, &again).unwrap();
    age(&again);
    assert!(env.scan().is_empty(), "identical content after ignoring");
    assert!(env.inbox.candidates().unwrap().is_empty());
}

#[test]
fn an_imported_download_is_not_offered_again_after_a_restart() {
    let mut env = Env::new();
    let zip = bandcamp_zip(&env);
    let id = env.scan()[0].id;
    env.inbox.accept(id, &mut |_, _| {}).unwrap();

    let mut reopened = Inbox::open(&env.root.join("ledger.db"), &env.library).unwrap();
    let again = env.downloads.join("Radiohead - OK Computer (1).zip");
    fs::copy(&zip, &again).unwrap();
    age(&again);

    assert!(reopened.scan(&[env.downloads.clone()]).unwrap().is_empty());
    assert_eq!(reopened.purchases().unwrap().len(), 1);
}

#[test]
fn different_content_under_the_same_name_is_a_new_download() {
    let mut env = Env::new();
    let zip = env.zip("Album.zip", &[("01 Song.flac", b"first version")]);
    assert_eq!(env.scan().len(), 1);

    write_zip(&zip, &[("01 Song.flac", b"second, different version")]);
    age(&zip);

    assert_eq!(env.scan().len(), 1);
    assert_eq!(env.inbox.candidates().unwrap().len(), 2);
}

#[test]
fn ignore_removes_a_candidate_for_good() {
    let mut env = Env::new();
    bandcamp_zip(&env);
    let id = env.scan()[0].id;

    env.inbox.ignore(id).unwrap();

    assert!(env.inbox.candidates().unwrap().is_empty());
    assert!(matches!(env.inbox.ignore(id), Err(Error::CandidateNotFound(found)) if found == id));
    assert!(matches!(
        env.inbox.accept(id, &mut |_, _| {}),
        Err(Error::CandidateNotFound(found)) if found == id
    ));
    assert!(matches!(
        env.inbox.ignore(9999),
        Err(Error::CandidateNotFound(9999))
    ));
}

#[test]
fn a_candidate_whose_zip_was_deleted_stops_being_offered() {
    let mut env = Env::new();
    let zip = bandcamp_zip(&env);
    let bytes = fs::read(&zip).unwrap();
    env.scan();

    fs::remove_file(&zip).unwrap();
    assert!(env.scan().is_empty());
    assert!(env.inbox.candidates().unwrap().is_empty());

    // Downloading it again later is a fresh offer.
    fs::write(&zip, bytes).unwrap();
    age(&zip);
    assert_eq!(env.scan().len(), 1);
}

#[test]
fn candidates_and_purchases_serialize_for_the_app() {
    let mut env = Env::new();
    bandcamp_zip(&env);
    let candidate = env.scan().remove(0);
    let json = serde_json::to_value(&candidate).unwrap();
    assert_eq!(json["store"], "bandcamp");
    assert_eq!(json["trackCount"], 2);
    assert!(json["foundAt"].is_i64());
    assert_eq!(
        serde_json::from_value::<Candidate>(json).unwrap(),
        candidate
    );

    let purchase = env.inbox.accept(candidate.id, &mut |_, _| {}).unwrap();
    let json = serde_json::to_value(&purchase).unwrap();
    assert_eq!(json["importedAt"], purchase.imported_at);
    assert_eq!(
        serde_json::from_value::<unsub_purchases::Purchase>(json).unwrap(),
        purchase
    );
    assert_eq!(
        serde_json::to_value(Store::SevenDigital).unwrap(),
        "sevenDigital"
    );
}

// ---------------------------------------------------------------------------------------------
// Accepting
// ---------------------------------------------------------------------------------------------

#[test]
fn accept_unpacks_into_artist_and_album_and_leaves_the_zip() {
    let mut env = Env::new();
    let mut entries = bandcamp_entries();
    entries.push(("Booklet.pdf".to_owned(), vec![4; 300]));
    entries.push(("__MACOSX/._cover.jpg".to_owned(), vec![5; 10]));
    let borrowed: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    let zip = env.zip(&format!("{BANDCAMP}.zip"), &borrowed);
    let zip_bytes = fs::read(&zip).unwrap();
    let id = env.scan()[0].id;

    let mut progress = Vec::new();
    let purchase = env
        .inbox
        .accept(id, &mut |done, total| progress.push((done, total)))
        .unwrap();

    let album = env.library.join("Radiohead").join("OK Computer");
    assert_eq!(
        purchase.files,
        [
            album.join("Radiohead - OK Computer - 01 Airbag.flac"),
            album.join("Radiohead - OK Computer - 02 Paranoid Android.flac"),
            album.join("cover.jpg"),
        ]
    );
    assert_eq!(fs::read(&purchase.files[0]).unwrap(), vec![1; 4000]);
    assert_eq!(fs::read(&purchase.files[2]).unwrap(), vec![3; 500]);
    assert_eq!(fs::read(album.join("Booklet.pdf")).unwrap(), vec![4; 300]);
    assert_eq!(purchase.store, Store::Bandcamp);
    assert_eq!(purchase.artist, "Radiohead");
    assert_eq!(purchase.album, "OK Computer");
    assert_eq!(purchase.source, zip);
    assert!(purchase.imported_at > 1_700_000_000);

    assert_eq!(fs::read(&zip).unwrap(), zip_bytes, "the zip is untouched");
    assert_eq!(
        env.library_contents(),
        [
            "Radiohead",
            "Radiohead/OK Computer",
            "Radiohead/OK Computer/Booklet.pdf",
            "Radiohead/OK Computer/Radiohead - OK Computer - 01 Airbag.flac",
            "Radiohead/OK Computer/Radiohead - OK Computer - 02 Paranoid Android.flac",
            "Radiohead/OK Computer/cover.jpg",
        ],
        "no staging folder, no junk"
    );

    // Progress runs from nothing to everything that was written, never backwards.
    assert_eq!(progress.first(), Some(&(0, 10_800)));
    assert_eq!(progress.last(), Some(&(10_800, 10_800)));
    assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));

    assert!(env.inbox.candidates().unwrap().is_empty());
    assert_eq!(env.inbox.purchases().unwrap(), [purchase]);
}

#[test]
fn accept_flattens_the_folder_a_store_wraps_around_the_album() {
    let mut env = Env::new();
    let name = "Björk - Homogenic (1997) [FLAC] [24B-96kHz]";
    env.zip(
        &format!("{name}.zip"),
        &[
            (&format!("{name}/CD 1/01 - Hunter.flac"), &[1; 100]),
            (&format!("{name}/CD 2/01 - Bonus.flac"), &[2; 100]),
            (&format!("{name}/cover.png"), &[3; 100]),
        ],
    );
    let id = env.scan()[0].id;

    let purchase = env.inbox.accept(id, &mut |_, _| {}).unwrap();

    let album = env.library.join("Björk").join("Homogenic");
    assert_eq!(
        purchase.files,
        [
            album.join("CD 1/01 - Hunter.flac"),
            album.join("CD 2/01 - Bonus.flac"),
            album.join("cover.png"),
        ]
    );
}

#[test]
fn accept_works_on_deflated_zips() {
    let mut env = Env::new();
    let path = env.downloads.join("Album.zip");
    let song = vec![9u8; 50_000];
    write_zip_with(
        &path,
        &[("01 Song.flac", &song)],
        CompressionMethod::Deflated,
    );
    age(&path);
    let id = env.scan()[0].id;

    let purchase = env.inbox.accept(id, &mut |_, _| {}).unwrap();

    assert_eq!(fs::read(&purchase.files[0]).unwrap(), song);
}

#[test]
fn accept_never_overwrites_an_existing_album() {
    let mut env = Env::new();
    let existing = env.library.join("Radiohead/OK Computer");
    fs::create_dir_all(&existing).unwrap();
    let mine = existing.join("Radiohead - OK Computer - 01 Airbag.flac");
    fs::write(&mine, b"my own rip").unwrap();
    bandcamp_zip(&env);
    let id = env.scan()[0].id;

    let purchase = env.inbox.accept(id, &mut |_, _| {}).unwrap();

    assert_eq!(fs::read(&mine).unwrap(), b"my own rip");
    let second = env.library.join("Radiohead/OK Computer (2)");
    assert_eq!(
        purchase.files[0],
        second.join("Radiohead - OK Computer - 01 Airbag.flac")
    );
    assert_eq!(fs::read(&purchase.files[0]).unwrap(), vec![1; 4000]);
}

#[test]
fn accepting_the_same_album_twice_numbers_each_copy() {
    let mut env = Env::new();
    let id = {
        env.zip("Album.zip", &[("01 Song.flac", b"one")]);
        env.scan()[0].id
    };
    let first = env.inbox.accept(id, &mut |_, _| {}).unwrap();
    // A different download of an album with the same name.
    env.zip("Album v2.zip", &[("Album/01 Song.flac", b"two")]);
    let id = env.scan()[0].id;
    let second = env.inbox.accept(id, &mut |_, _| {}).unwrap();

    let (first, second) = (&first.files[0], &second.files[0]);
    assert_eq!(fs::read(first).unwrap(), b"one");
    assert_eq!(fs::read(second).unwrap(), b"two");
    assert_ne!(first.parent(), second.parent());
}

#[test]
fn accept_refuses_an_archive_that_would_escape_the_library() {
    let mut env = Env::new();
    for (zip_name, evil) in [
        ("parent.zip", "../evil.flac"),
        ("deep.zip", "Album/../../evil.flac"),
        ("absolute.zip", "/tmp/unsub-evil.flac"),
        ("backslash.zip", "..\\evil.flac"),
    ] {
        env.zip(zip_name, &[("01 Fine.flac", b"fine"), (evil, b"evil")]);
    }
    for candidate in env.scan() {
        let result = env.inbox.accept(candidate.id, &mut |_, _| {});

        assert!(
            matches!(result, Err(Error::UnsafeArchive(_))),
            "{:?}: {result:?}",
            candidate.path
        );
    }

    assert!(
        env.library_contents().is_empty(),
        "nothing at all was written"
    );
    assert!(!env.root.join("evil.flac").exists());
    assert!(!Path::new("/tmp/unsub-evil.flac").exists());
    assert_eq!(env.inbox.candidates().unwrap().len(), 4, "still pending");
    assert!(env.inbox.purchases().unwrap().is_empty());
}

#[test]
fn a_failure_midway_leaves_no_partial_folder() {
    let mut env = Env::new();
    let path = env.downloads.join("Album.zip");
    write_zip(
        &path,
        &[
            ("01 First.flac", &[b'A'; 2000]),
            ("02 Second.flac", &[b'B'; 2000]),
            ("03 Third.flac", &[b'C'; 2000]),
        ],
    );
    // Damage the third track's data so its checksum fails after the first two were written.
    let mut bytes = fs::read(&path).unwrap();
    let third = bytes
        .windows(2000)
        .position(|w| w.iter().all(|b| *b == b'C'))
        .unwrap();
    bytes[third + 1000] = b'X';
    fs::write(&path, bytes).unwrap();
    age(&path);
    let id = env.scan()[0].id;

    let result = env.inbox.accept(id, &mut |_, _| {});

    // The first two tracks were already on disk when the third failed its checksum.
    assert!(
        matches!(&result, Err(Error::Io(error)) if error.to_string().contains("checksum")),
        "{result:?}"
    );
    assert!(
        env.library_contents().is_empty(),
        "{:?}",
        env.library_contents()
    );
    assert_eq!(
        env.inbox.candidates().unwrap().len(),
        1,
        "it can be tried again"
    );
    assert!(env.inbox.purchases().unwrap().is_empty());
}

#[test]
fn a_failure_keeps_other_artists_folders_intact() {
    let mut env = Env::new();
    let other = env.library.join("Someone Else/Their Album");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("01.flac"), b"theirs").unwrap();
    env.zip(
        "Album.zip",
        &[("01 Song.flac", b"fine"), ("../evil.flac", b"evil")],
    );
    let id = env.scan()[0].id;

    assert!(env.inbox.accept(id, &mut |_, _| {}).is_err());

    assert_eq!(
        env.library_contents(),
        [
            "Someone Else",
            "Someone Else/Their Album",
            "Someone Else/Their Album/01.flac"
        ]
    );
}

#[test]
fn accept_copies_a_folder_and_leaves_the_original() {
    let mut env = Env::new();
    env.scan();
    let album = env.downloads.join("Fresh From The Store");
    folder(
        &album,
        &[
            ("01 Come Together.flac", BEATLES_FLAC),
            ("CD 2/02 Bonus.flac", b"bonus"),
            ("cover.jpg", b"jpg"),
            (".DS_Store", b"junk"),
        ],
    );
    let id = env.scan()[0].id;

    let mut last_progress = (0, 0);
    let purchase = env
        .inbox
        .accept(id, &mut |done, total| last_progress = (done, total))
        .unwrap();

    let destination = env.library.join("The Beatles/Abbey Road");
    assert_eq!(
        purchase.files,
        [
            destination.join("01 Come Together.flac"),
            destination.join("CD 2/02 Bonus.flac"),
            destination.join("cover.jpg"),
        ]
    );
    assert_eq!(fs::read(&purchase.files[0]).unwrap(), BEATLES_FLAC);
    assert!(!destination.join(".DS_Store").exists());
    assert_eq!(purchase.source, album);
    assert!(
        album.join("01 Come Together.flac").exists(),
        "the original stays"
    );
    let total = BEATLES_FLAC.len() as u64 + 5 + 3;
    assert_eq!(last_progress, (total, total));
}

#[test]
fn a_folder_entry_named_like_a_track_is_not_a_track() {
    let mut env = Env::new();
    let path = env.downloads.join("Odd.zip");
    let mut writer = ZipWriter::new(File::create(&path).unwrap());
    writer
        .add_directory("01.flac/", SimpleFileOptions::default())
        .unwrap();
    writer
        .start_file("cover.jpg", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"jpg").unwrap();
    writer.finish().unwrap();
    age(&path);

    assert!(env.scan().is_empty());
}

#[test]
fn purchases_are_listed_newest_first_and_survive_a_restart() {
    let mut env = Env::new();
    env.zip("Alpha.zip", &[("01 Song.flac", b"a")]);
    let alpha = env.scan()[0].id;
    env.inbox.accept(alpha, &mut |_, _| {}).unwrap();
    env.zip("Beta.zip", &[("01 Song.flac", b"b")]);
    let beta = env.scan()[0].id;
    env.inbox.accept(beta, &mut |_, _| {}).unwrap();

    let reopened = Inbox::open(&env.root.join("ledger.db"), &env.library).unwrap();
    let albums: Vec<_> = reopened
        .purchases()
        .unwrap()
        .into_iter()
        .map(|purchase| purchase.album)
        .collect();

    assert_eq!(albums, ["Beta", "Alpha"]);
}
