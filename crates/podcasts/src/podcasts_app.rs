//! The shows followed in the Podcasts app on a Mac, read from its library so they can be
//! subscribed to here. The app's database is only ever read from a private copy: the app may have
//! it open, and nothing here may lock or change it.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::Connection;

use crate::directory::{Listing, image_url};
use crate::error::{Error, Result};

const LIBRARY_RELATIVE_PATH: &str =
    "Library/Group Containers/243LU875E5.groups.com.apple.podcasts/Documents/MTLibrary.sqlite";

/// Present when the library tells followed shows from ones that were only browsed or removed.
const FOLLOWED_COLUMN: &str = "ZSUBSCRIBED";
const ARTWORK_COLUMN: &str = "ZIMAGEURL";
const AUTHOR_COLUMN: &str = "ZAUTHOR";

pub fn library_path(home: &Path) -> PathBuf {
    home.join(LIBRARY_RELATIVE_PATH)
}

/// Followed shows by title, case-insensitively. Column names vary between macOS versions, so the
/// table is inspected first; only the feed address and title are required.
pub fn followed_shows(library: &Path) -> Result<Vec<Listing>> {
    let copy = tempfile::tempdir()?;
    let database = copy.path().join("MTLibrary.sqlite");
    copy_library(library, &database)?;
    let connection = Connection::open(&database)?;

    let columns = connection
        .prepare("SELECT name FROM pragma_table_info('ZMTPODCAST')")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has = |name: &str| columns.iter().any(|column| column == name);
    if !has("ZFEEDURL") || !has("ZTITLE") {
        return Err(Error::PodcastsAppLayout);
    }

    // Each of these names was just read from the table, so none is user input.
    let optional = |name: &'static str| if has(name) { name } else { "NULL" };
    let filter = if has(FOLLOWED_COLUMN) {
        format!("AND {FOLLOWED_COLUMN} = 1")
    } else {
        String::new()
    };
    let sql = format!(
        "SELECT ZFEEDURL, ZTITLE, {}, {} FROM ZMTPODCAST \
         WHERE ZFEEDURL IS NOT NULL AND TRIM(ZFEEDURL) <> '' {filter} \
         ORDER BY COALESCE(ZTITLE, ZFEEDURL) COLLATE NOCASE",
        optional(AUTHOR_COLUMN),
        optional(ARTWORK_COLUMN),
    );
    let mut shows: Vec<Listing> = Vec::new();
    let rows = connection
        .prepare(&sql)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (feed_url, title, author, artwork_url) in rows {
        let feed_url = feed_url.trim().to_owned();
        if shows.iter().any(|show| show.feed_url == feed_url) {
            continue;
        }
        shows.push(Listing {
            title: title
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| feed_url.clone()),
            feed_url,
            author: author.unwrap_or_default(),
            artwork_url: image_url(artwork_url),
            genre: String::new(),
            episode_count: None,
        });
    }
    Ok(shows)
}

/// Size and modification time, or `None` for a file that isn't there.
type Stamp = Option<(u64, Option<SystemTime>)>;

fn stamp(path: &Path) -> Result<Stamp> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some((metadata.len(), metadata.modified().ok()))),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(app_error(err)),
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// Copies the database and, if the app hasn't folded it in yet, its write-ahead log, which holds
/// the newest changes.
fn copy_library(library: &Path, database: &Path) -> Result<()> {
    let log = sibling(library, "-wal");
    let stamps = || Ok::<_, Error>((stamp(library)?, stamp(&log)?));
    let before = stamps()?;
    copy_once(library, database)?;
    // The app can write while the two files are copied one after the other, which leaves a copy
    // that mixes two states of the library. Files that changed meanwhile are copied again.
    if stamps()? != before {
        copy_once(library, database)?;
    }
    Ok(())
}

fn copy_once(library: &Path, database: &Path) -> Result<()> {
    fs::copy(library, database).map_err(app_error)?;
    let log_copy = sibling(database, "-wal");
    match fs::copy(sibling(library, "-wal"), &log_copy) {
        // A log left from an earlier pass would be replayed over a newer database.
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let _ = fs::remove_file(&log_copy);
            Ok(())
        }
        other => other.map(drop).map_err(app_error),
    }
}

fn app_error(err: std::io::Error) -> Error {
    match err.kind() {
        ErrorKind::NotFound => Error::PodcastsAppMissing,
        ErrorKind::PermissionDenied => Error::PodcastsAppDenied,
        _ => Error::Io(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library laid out as the app's is, with only the columns asked for.
    fn make_library(dir: &Path, columns: &str, rows: &[&str]) -> PathBuf {
        let path = dir.join("MTLibrary.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE ZMTPODCAST (Z_PK INTEGER PRIMARY KEY, {columns});"
            ))
            .unwrap();
        for row in rows {
            connection
                .execute_batch(&format!("INSERT INTO ZMTPODCAST VALUES ({row});"))
                .unwrap();
        }
        path
    }

    fn titles(shows: &[Listing]) -> Vec<&str> {
        shows.iter().map(|show| show.title.as_str()).collect()
    }

    const FULL_COLUMNS: &str =
        "ZFEEDURL TEXT, ZTITLE TEXT, ZAUTHOR TEXT, ZIMAGEURL TEXT, ZSUBSCRIBED INTEGER";

    #[test]
    fn only_followed_shows_with_a_feed_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let path = make_library(
            dir.path(),
            FULL_COLUMNS,
            &[
                "1, 'https://b.example/rss', 'banana talk', 'Bea', 'https://img.example/b.jpg', 1",
                "2, 'https://a.example/rss', 'Apple Corps', NULL, 'http://img.example/a.jpg', 1",
                "3, 'https://c.example/rss', 'Unfollowed', 'Cy', NULL, 0",
                "4, NULL, 'Store-only show', NULL, NULL, 1",
                "5, '  ', 'Blank feed', NULL, NULL, 1",
                "6, ' https://a.example/rss ', 'Apple Corps again', NULL, NULL, 1",
                "7, 'https://d.example/rss', NULL, NULL, NULL, 1",
            ],
        );
        let before = fs::read(&path).unwrap();

        let shows = followed_shows(&path).unwrap();

        assert_eq!(
            titles(&shows),
            ["Apple Corps", "banana talk", "https://d.example/rss"]
        );
        assert_eq!(shows[0].feed_url, "https://a.example/rss");
        assert_eq!(shows[0].author, "");
        assert_eq!(
            shows[0].artwork_url.as_deref(),
            Some("http://img.example/a.jpg")
        );
        assert_eq!(shows[1].author, "Bea");
        assert_eq!(
            shows[1].artwork_url.as_deref(),
            Some("https://img.example/b.jpg")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn without_a_followed_column_every_show_with_a_feed_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let path = make_library(
            dir.path(),
            "ZFEEDURL TEXT, ZTITLE TEXT",
            &[
                "1, 'https://one.example/rss', 'One'",
                "2, 'https://two.example/rss', 'Two'",
                "3, NULL, 'Three'",
            ],
        );
        let shows = followed_shows(&path).unwrap();
        assert_eq!(titles(&shows), ["One", "Two"]);
        assert_eq!(shows[0].author, "");
        assert_eq!(shows[0].artwork_url, None);
    }

    #[test]
    fn a_library_without_the_needed_columns_is_not_recognised() {
        let dir = tempfile::tempdir().unwrap();
        let path = make_library(dir.path(), "ZTITLE TEXT", &["1, 'One'"]);
        assert!(matches!(
            followed_shows(&path),
            Err(Error::PodcastsAppLayout)
        ));
    }

    #[test]
    fn changes_still_in_the_write_ahead_log_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = make_library(dir.path(), FULL_COLUMNS, &[]);
        // Like the running app: WAL mode, with the connection (and so the log) still open.
        let app = Connection::open(&path).unwrap();
        app.pragma_update(None, "journal_mode", "WAL").unwrap();
        app.execute_batch("INSERT INTO ZMTPODCAST VALUES (1, 'https://new.example/rss', 'Just Followed', NULL, NULL, 1);")
            .unwrap();

        let shows = followed_shows(&path).unwrap();

        assert_eq!(titles(&shows), ["Just Followed"]);
    }

    #[test]
    fn a_missing_library_is_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let result = followed_shows(&dir.path().join("MTLibrary.sqlite"));
        assert!(matches!(result, Err(Error::PodcastsAppMissing)));
    }

    #[cfg(unix)]
    #[test]
    fn a_library_that_may_not_be_read_is_reported_as_such() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = make_library(dir.path(), FULL_COLUMNS, &[]);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = followed_shows(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(result, Err(Error::PodcastsAppDenied)));
    }

    #[test]
    fn the_library_lives_in_the_apps_group_container() {
        assert_eq!(
            library_path(Path::new("/Users/me")),
            Path::new(
                "/Users/me/Library/Group Containers/243LU875E5.groups.com.apple.podcasts/Documents/MTLibrary.sqlite"
            )
        );
    }
}
