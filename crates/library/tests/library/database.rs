use rusqlite::Connection;
use tempfile::TempDir;
use unsub_library::{Error, Library};

use crate::common::{all_kinds, copy_fixtures, rewind_schema, scan};

#[test]
fn open_creates_parent_directories_and_persists() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("nested/app/library.db");
    let fixtures = copy_fixtures();

    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
    }

    let reopened = Library::open(&db_path).unwrap();
    assert_eq!(reopened.stats(&all_kinds()).unwrap().count, 8);
}

#[test]
fn open_enables_wal_and_stamps_the_schema_version() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    drop(Library::open(&db_path).unwrap());

    let conn = Connection::open(&db_path).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();

    assert_eq!(mode, "wal");
    assert_eq!(version, 13);
}

#[test]
fn open_rejects_a_database_from_a_newer_build() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    drop(Library::open(&db_path).unwrap());
    Connection::open(&db_path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();

    assert!(matches!(
        Library::open(&db_path),
        Err(Error::SchemaTooNew {
            found: 99,
            supported: 13
        })
    ));
}

#[test]
fn migration_stamps_existing_rows_with_parser_version_one_so_they_are_reread() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    let fixtures = copy_fixtures();
    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
    }
    rewind_schema(&db_path, 1);

    let mut library = Library::open(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    let parser_versions: Vec<u32> = conn
        .prepare("SELECT DISTINCT parser_version FROM tracks")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(version, 13);
    assert_eq!(parser_versions, [1]);
    assert_eq!(
        library.stats(&all_kinds()).unwrap().count,
        8,
        "rows survive the migration"
    );

    let summary = library
        .add_paths(&[fixtures.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!((summary.updated, summary.unchanged), (8, 0));
    let oldest: u32 = conn
        .query_row("SELECT MIN(parser_version) FROM tracks", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(oldest > 1);
}

#[test]
fn migration_twelve_adds_the_rescued_table_to_a_version_eleven_database_and_keeps_its_rows() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    let fixtures = copy_fixtures();
    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
    }
    rewind_schema(&db_path, 11);
    let version = |db_path: &std::path::Path| -> u32 {
        Connection::open(db_path)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    };
    let has_table = |db_path: &std::path::Path| -> bool {
        Connection::open(db_path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'rescued'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
            == 1
    };
    assert_eq!((version(&db_path), has_table(&db_path)), (11, false));

    let library = Library::open(&db_path).unwrap();

    assert_eq!((version(&db_path), has_table(&db_path)), (13, true));
    assert_eq!(library.stats(&all_kinds()).unwrap().count, 8);
    assert!(library.rescued(&["ipod:1".to_owned()]).unwrap().is_empty());
}

#[test]
fn migration_thirteen_adds_the_unreadable_songs_table_to_a_version_twelve_database() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    drop(Library::open(&db_path).unwrap());
    rewind_schema(&db_path, 12);
    let has_table = |db_path: &std::path::Path| -> bool {
        Connection::open(db_path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'unreadable_songs'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
            == 1
    };
    assert!(!has_table(&db_path));

    drop(Library::open(&db_path).unwrap());

    assert!(has_table(&db_path));
}

#[test]
fn a_database_is_copied_aside_once_before_it_gets_schema_thirteen() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    let backup = root.path().join("library.db.before-13");
    let fixtures = copy_fixtures();
    {
        let mut library = Library::open(&db_path).unwrap();
        assert!(!backup.exists(), "a fresh database has nothing to keep");
        scan(&mut library, fixtures.path());
    }
    rewind_schema(&db_path, 12);

    drop(Library::open(&db_path).unwrap());

    // The copy is the library as an older build knows it, with its songs.
    let copy = Connection::open(&backup).unwrap();
    let version: u32 = copy
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let songs: u32 = copy
        .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
        .unwrap();
    let has_new_table: u32 = copy
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'unreadable_songs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((version, songs, has_new_table), (12, 8, 0));
    drop(copy);

    // Kept once: a later migration does not replace it.
    std::fs::write(&backup, b"kept").unwrap();
    rewind_schema(&db_path, 12);
    drop(Library::open(&db_path).unwrap());
    assert_eq!(std::fs::read(&backup).unwrap(), b"kept");
}

#[test]
fn a_copy_a_crash_left_half_made_is_not_taken_for_the_backup() {
    let root = TempDir::new().unwrap();
    let db_path = root.path().join("library.db");
    let backup = root.path().join("library.db.before-13");
    let partial = root.path().join("library.db.before-13.tmp");
    let fixtures = copy_fixtures();
    {
        let mut library = Library::open(&db_path).unwrap();
        scan(&mut library, fixtures.path());
    }
    rewind_schema(&db_path, 12);
    std::fs::write(&partial, b"half a database").unwrap();

    drop(Library::open(&db_path).unwrap());

    assert!(!partial.exists());
    let songs: u32 = Connection::open(&backup)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(songs, 8, "a whole copy was made over it");
}
