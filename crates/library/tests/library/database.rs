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
    assert_eq!(version, 10);
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
            supported: 10
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
    assert_eq!(version, 10);
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
