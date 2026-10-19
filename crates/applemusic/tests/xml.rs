use std::fs;
use std::path::{Path, PathBuf};

use antistreaming_applemusic::{Error, MediaType, MusicLibraryItem, Ownership, read_xml};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use tempfile::TempDir;

/// Files the fixture's "exists" locations point at, relative to the staged media folder.
const MEDIA_FILES: &[&str] = &[
    "Radiohead/OK Computer/01 Airbag.mp3",
    "Björk/Homogenic/01 Hunter.m4a",
    "Eagles/Hotel California/01 Hotel California.m4p",
    "Daft Punk/Discovery/02 Aerodynamic.m4a",
    "Taylor Swift/Lover/03 Lover.m4p",
    "Podcasts/Radiolab/Episode 12.mp3",
    "Videos/Take On Me.m4v",
    "Audiobooks/Dune/01.m4b",
    "Bought/Purchased By Kind.m4a",
];

const KEEP_SLASHES: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/');

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// `fixtures/library.xml` with its `@MEDIA@` placeholder pointed at real files in a temp folder,
/// since "the file exists" can only be tested against paths that exist on the test machine.
struct Staged {
    _dir: TempDir,
    media: PathBuf,
    items: Vec<MusicLibraryItem>,
}

impl Staged {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        for relative in MEDIA_FILES {
            let file = media.join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, b"").unwrap();
        }

        // Locations are file URLs, so a Windows temp path needs forward slashes and a leading one.
        let mut url_path = media.to_str().unwrap().replace('\\', "/");
        if !url_path.starts_with('/') {
            url_path.insert(0, '/');
        }
        let encoded = utf8_percent_encode(&url_path, KEEP_SLASHES).to_string();
        let export = dir.path().join("library.xml");
        let template = fs::read_to_string(fixture("library.xml")).unwrap();
        fs::write(&export, template.replace("@MEDIA@", &encoded)).unwrap();

        let items = read_xml(&export).unwrap();
        Self {
            _dir: dir,
            media,
            items,
        }
    }

    fn item(&self, persistent_id: &str) -> &MusicLibraryItem {
        self.items
            .iter()
            .find(|item| item.persistent_id == persistent_id)
            .unwrap_or_else(|| panic!("no item {persistent_id}"))
    }
}

fn id(suffix: &str) -> String {
    format!("A1B2C3D4E5F607{suffix}")
}

#[test]
fn classifies_every_ownership_path() {
    use Ownership::*;
    let staged = Staged::new();
    let expected = [
        ("01", LocalFile, "ripped MP3"),
        ("02", LocalFile, "purchased and downloaded"),
        ("03", LocalFile, "old FairPlay purchase"),
        ("04", PurchasedInCloud, "purchased, not downloaded"),
        ("05", LocalFile, "matched and downloaded"),
        ("06", UploadedInCloud, "matched, not downloaded"),
        ("07", Subscription, "Apple Music, streamed"),
        ("08", Subscription, "Apple Music, downloaded"),
        ("09", MissingFile, "file missing, no cloud flags"),
        ("0A", UploadedInCloud, "matched, file missing"),
        ("0B", MissingFile, "Windows path on this machine"),
        ("0C", LocalFile, "podcast file"),
        ("0D", Unknown, "podcast stream"),
        ("0E", LocalFile, "music video file"),
        ("0F", PurchasedInCloud, "purchased movie"),
        ("10", Unknown, "TV show without a file"),
        ("11", LocalFile, "audiobook file"),
        ("12", Unknown, "bare entry"),
        ("13", Unknown, "protected, not purchased, not Apple Music"),
        ("14", Subscription, "Apple Music by kind only"),
        ("15", MissingFile, "unmounted volume"),
        (
            "16",
            PurchasedInCloud,
            "purchased by kind only, file missing",
        ),
        ("17", LocalFile, "purchased by kind only, downloaded"),
    ];
    for (suffix, ownership, case) in expected {
        assert_eq!(staged.item(&id(suffix)).ownership, ownership, "{case}");
    }
    assert_eq!(staged.items.len(), expected.len());
}

#[test]
fn reads_every_field_of_a_full_entry() {
    let staged = Staged::new();
    assert_eq!(
        staged.item(&id("01")),
        &MusicLibraryItem {
            persistent_id: id("01"),
            title: "Airbag".into(),
            artist: "Radiohead".into(),
            album_artist: "Radiohead".into(),
            album: "OK Computer".into(),
            genre: "Alternative".into(),
            duration_ms: Some(287_000),
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(1997),
            kind: "MPEG audio file".into(),
            media: MediaType::Music,
            ownership: Ownership::LocalFile,
            location: Some(staged.media.join("Radiohead/OK Computer/01 Airbag.mp3")),
            file_present: true,
            play_count: 12,
            rating: 80,
            date_added: Some(1_552_314_125),
        }
    );
}

#[test]
fn fills_defaults_for_missing_optional_keys() {
    let staged = Staged::new();
    assert_eq!(
        staged.item(&id("12")),
        &MusicLibraryItem {
            persistent_id: id("12"),
            title: "Untitled".into(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            genre: String::new(),
            duration_ms: None,
            track_number: None,
            disc_number: None,
            year: None,
            kind: String::new(),
            media: MediaType::Music,
            ownership: Ownership::Unknown,
            location: None,
            file_present: false,
            play_count: 0,
            rating: 0,
            date_added: None,
        }
    );
}

#[test]
fn decodes_percent_encoded_paths_with_spaces_and_non_ascii() {
    let staged = Staged::new();
    assert_eq!(
        staged.item(&id("02")).location,
        Some(staged.media.join("Björk/Homogenic/01 Hunter.m4a"))
    );
    assert_eq!(
        staged.item(&id("0C")).location,
        Some(staged.media.join("Podcasts/Radiolab/Episode 12.mp3"))
    );
}

#[test]
fn location_keeps_the_recorded_path_when_the_file_is_gone() {
    let staged = Staged::new();
    let missing = staged.item(&id("09"));
    assert_eq!(
        missing.location,
        Some(staged.media.join("Missing/Gone.mp3"))
    );
    assert!(!missing.file_present);

    let unmounted = staged.item(&id("15"));
    assert_eq!(
        unmounted.location,
        Some(PathBuf::from(
            "/Volumes/EXTERNALNEW/Music/Unmounted Volume.mp3"
        ))
    );
    assert!(!unmounted.file_present);

    // A Windows drive path is recorded as written, though it cannot exist on this machine.
    let windows = staged.item(&id("0B"));
    assert_eq!(
        windows.location,
        Some(PathBuf::from(
            "C:/Users/riley/Music/iTunes/iTunes Media/Music/Nirvana/Nevermind/01 Smells Like Teen Spirit.mp3"
        ))
    );
    assert!(!windows.file_present);
}

#[test]
fn file_present_follows_the_disk() {
    let staged = Staged::new();
    // A subscription download is present even though it is not owned.
    for suffix in ["01", "02", "08", "0C", "17"] {
        assert!(staged.item(&id(suffix)).file_present, "{suffix}");
    }
    for suffix in ["04", "09", "0A", "0B", "16"] {
        assert!(!staged.item(&id(suffix)).file_present, "{suffix}");
    }
}

#[test]
fn entries_without_a_file_url_have_no_location() {
    let staged = Staged::new();
    // A podcast stream, a bare entry and a TV show with no Location at all.
    for suffix in ["0D", "10", "12"] {
        let item = staged.item(&id(suffix));
        assert_eq!(item.location, None, "{suffix}");
        assert!(!item.file_present, "{suffix}");
    }
}

#[test]
fn maps_media_types_from_flags() {
    use MediaType::*;
    let staged = Staged::new();
    for (suffix, media) in [
        ("01", Music),
        ("0C", Podcast),
        ("0D", Podcast),
        ("0E", Video),
        ("0F", Video),
        ("10", Video),
        ("11", Audiobook),
    ] {
        assert_eq!(staged.item(&id(suffix)).media, media, "{suffix}");
    }
}

#[test]
fn uppercases_persistent_ids_and_skips_entries_without_one() {
    let staged = Staged::new();
    // Stored as lowercase hex in the fixture.
    assert_eq!(staged.item(&id("09")).title, "Gone");
    assert!(
        staged
            .items
            .iter()
            .all(|item| item.title != "No Persistent ID")
    );
}

#[test]
fn keeps_the_order_of_the_export() {
    let staged = Staged::new();
    // The fixture lists tracks in ascending persistent-ID order.
    let ids: Vec<_> = staged
        .items
        .iter()
        .map(|item| &item.persistent_id)
        .collect();
    assert!(ids.is_sorted());
}

#[test]
fn serializes_with_camel_case_keys() {
    let staged = Staged::new();
    let json = serde_json::to_value(staged.item(&id("04"))).unwrap();
    assert_eq!(json["persistentId"], id("04"));
    assert_eq!(json["ownership"], "purchasedInCloud");
    assert_eq!(json["media"], "music");
    assert_eq!(json["albumArtist"], "");
    assert_eq!(json["durationMs"], serde_json::Value::Null);
    assert_eq!(json["filePresent"], false);

    let json = serde_json::to_value(staged.item(&id("09"))).unwrap();
    assert_eq!(json["ownership"], "missingFile");
}

#[test]
fn round_trips_through_json() {
    let staged = Staged::new();
    // Every ownership and media variant the fixture produces, with and without optional fields.
    let json = serde_json::to_string(&staged.items).unwrap();
    let restored: Vec<MusicLibraryItem> = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, staged.items);
}

#[test]
fn export_without_tracks_is_empty() {
    assert!(read_xml(&fixture("no-tracks.xml")).unwrap().is_empty());
}

#[test]
fn malformed_plist_is_a_plist_error() {
    assert!(matches!(
        read_xml(&fixture("malformed.xml")),
        Err(Error::Plist(_))
    ));
}

#[test]
fn missing_export_is_an_io_error() {
    assert!(matches!(
        read_xml(&fixture("does-not-exist.xml")),
        Err(Error::Io(_))
    ));
}
