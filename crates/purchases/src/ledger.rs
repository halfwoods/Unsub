use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{Error, Result};
use crate::model::{Candidate, Purchase, Store, StoreLink};

/// Appended to, never edited: `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &["
    -- Everything the scanner has examined, so nothing is hashed or offered twice.
    CREATE TABLE sources (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        kind TEXT NOT NULL,
        size INTEGER NOT NULL,
        modified INTEGER NOT NULL,
        -- NULL for archives with no audio, which are never worth hashing.
        hash TEXT,
        status TEXT NOT NULL,
        store TEXT NOT NULL,
        artist TEXT NOT NULL,
        album TEXT NOT NULL,
        track_count INTEGER NOT NULL,
        formats TEXT NOT NULL,
        found_at INTEGER NOT NULL
    );
    CREATE INDEX sources_by_path ON sources (path);
    CREATE INDEX sources_by_hash ON sources (hash);
    CREATE TABLE purchases (
        id INTEGER PRIMARY KEY,
        source_id INTEGER NOT NULL REFERENCES sources (id),
        store TEXT NOT NULL,
        artist TEXT NOT NULL,
        album TEXT NOT NULL,
        files TEXT NOT NULL,
        imported_at INTEGER NOT NULL
    );
    -- Folders already in a watched directory the first time it is scanned were not downloaded
    -- since the app started looking, so they are not offered.
    CREATE TABLE watched (path TEXT PRIMARY KEY);
    CREATE TABLE link_cache (
        key TEXT PRIMARY KEY,
        links TEXT NOT NULL,
        fetched_at INTEGER NOT NULL
    );
"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// Offered to the user and waiting.
    Pending,
    Ignored,
    Imported,
    /// A folder that was already there when its directory was first scanned.
    Baseline,
    /// The same bytes as a source already seen.
    Duplicate,
    /// An archive with no audio in it.
    Rejected,
    /// Was pending, then the user deleted it.
    Gone,
}

impl Status {
    fn as_db(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ignored => "ignored",
            Self::Imported => "imported",
            Self::Baseline => "baseline",
            Self::Duplicate => "duplicate",
            Self::Rejected => "rejected",
            Self::Gone => "gone",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Zip,
    Folder,
}

impl Kind {
    fn as_db(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Folder => "folder",
        }
    }

    fn from_db(text: &str) -> Self {
        if text == "folder" {
            Self::Folder
        } else {
            Self::Zip
        }
    }
}

/// One row of `sources`.
pub(crate) struct Source {
    pub id: i64,
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub modified: i64,
    pub hash: Option<String>,
    pub status: Status,
    pub store: Store,
    pub artist: String,
    pub album: String,
    pub track_count: u32,
    pub formats: Vec<String>,
    pub found_at: i64,
}

impl Source {
    /// A row that records having looked at `path` without describing an album.
    pub(crate) fn bare(path: &Path, kind: Kind, size: u64, modified: i64, status: Status) -> Self {
        Self {
            id: 0,
            path: path.to_owned(),
            kind,
            size,
            modified,
            hash: None,
            status,
            store: Store::Unknown,
            artist: String::new(),
            album: String::new(),
            track_count: 0,
            formats: Vec::new(),
            found_at: unix_seconds(),
        }
    }

    pub(crate) fn candidate(&self) -> Candidate {
        Candidate {
            id: self.id,
            path: self.path.clone(),
            store: self.store,
            artist: self.artist.clone(),
            album: self.album.clone(),
            track_count: self.track_count,
            bytes: self.size,
            formats: self.formats.clone(),
            found_at: self.found_at,
        }
    }

    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let formats: String = row.get("formats")?;
        Ok(Self {
            id: row.get("id")?,
            path: PathBuf::from(row.get::<_, String>("path")?),
            kind: Kind::from_db(&row.get::<_, String>("kind")?),
            size: row.get("size")?,
            modified: row.get("modified")?,
            hash: row.get("hash")?,
            // Only pending rows are ever read back whole.
            status: Status::Pending,
            store: Store::from_db(&row.get::<_, String>("store")?),
            artist: row.get("artist")?,
            album: row.get("album")?,
            track_count: row.get("track_count")?,
            formats: formats
                .split(',')
                .filter(|format| !format.is_empty())
                .map(str::to_owned)
                .collect(),
            found_at: row.get("found_at")?,
        })
    }
}

const SOURCE_COLUMNS: &str = "id, path, kind, size, modified, hash, status, store, artist, album, track_count, formats, found_at";

/// The ledger of downloads seen, offered, ignored and imported, plus the cache of where-to-buy
/// answers. All of it lives in one SQLite file.
pub struct Inbox {
    pub(crate) conn: Connection,
    pub(crate) library_dir: PathBuf,
}

impl Inbox {
    /// Opens the ledger, creating it if needed. Accepted downloads are written under
    /// `library_dir`, which is created when the first one is.
    pub fn open(ledger_path: &Path, library_dir: &Path) -> Result<Inbox> {
        let mut conn = Connection::open(ledger_path)?;
        migrate(&mut conn)?;
        Ok(Inbox {
            conn,
            library_dir: library_dir.to_owned(),
        })
    }

    /// Downloads waiting for a decision, newest first.
    pub fn candidates(&self) -> Result<Vec<Candidate>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {SOURCE_COLUMNS} FROM sources WHERE status = 'pending'
             ORDER BY found_at DESC, id DESC"
        ))?;
        let sources = statement.query_map([], Source::from_row)?;
        sources.map(|source| Ok(source?.candidate())).collect()
    }

    /// Stops offering a download. The ledger keeps its hash, so the same file downloaded again
    /// stays hidden.
    pub fn ignore(&mut self, candidate_id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE sources SET status = ?1 WHERE id = ?2 AND status = 'pending'",
            params![Status::Ignored.as_db(), candidate_id],
        )?;
        if changed == 0 {
            return Err(Error::CandidateNotFound(candidate_id));
        }
        Ok(())
    }

    /// Everything accepted so far, newest first.
    pub fn purchases(&self) -> Result<Vec<Purchase>> {
        let mut statement = self.conn.prepare(
            "SELECT p.id, p.store, p.artist, p.album, p.files, p.imported_at, s.path
             FROM purchases p JOIN sources s ON s.id = p.source_id
             ORDER BY p.imported_at DESC, p.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                Purchase {
                    id: row.get(0)?,
                    store: Store::from_db(&row.get::<_, String>(1)?),
                    artist: row.get(2)?,
                    album: row.get(3)?,
                    files: Vec::new(),
                    imported_at: row.get(5)?,
                    source: PathBuf::from(row.get::<_, String>(6)?),
                },
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (mut purchase, files) = row?;
            purchase.files = serde_json::from_str(&files)?;
            Ok(purchase)
        })
        .collect()
    }

    pub(crate) fn pending(&self, candidate_id: i64) -> Result<Source> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {SOURCE_COLUMNS} FROM sources WHERE id = ?1 AND status = 'pending'"
                ),
                [candidate_id],
                Source::from_row,
            )
            .optional()?
            .ok_or(Error::CandidateNotFound(candidate_id))
    }

    pub(crate) fn insert_source(&self, source: &Source) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO sources (path, kind, size, modified, hash, status, store, artist, album,
                                  track_count, formats, found_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                path_text(&source.path)?,
                source.kind.as_db(),
                source.size,
                source.modified,
                source.hash,
                source.status.as_db(),
                source.store.as_db(),
                source.artist,
                source.album,
                source.track_count,
                source.formats.join(","),
                source.found_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Whether this exact file, unchanged, has been examined before.
    pub(crate) fn knows_file(&self, path: &Path, size: u64, modified: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sources
                            WHERE path = ?1 AND kind = 'zip' AND size = ?2 AND modified = ?3)",
            params![path_text(path)?, size, modified],
            |row| row.get(0),
        )?)
    }

    /// Whether content with this hash was already offered, ignored, imported or baselined.
    pub(crate) fn has_hash(&self, hash: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sources WHERE hash = ?1
                            AND status IN ('pending', 'ignored', 'imported', 'baseline'))",
            [hash],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn is_watched(&self, dir: &Path) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM watched WHERE path = ?1)",
            [path_text(dir)?],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn mark_watched(&self, dir: &Path) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO watched (path) VALUES (?1)",
            [path_text(dir)?],
        )?;
        Ok(())
    }

    /// Pending downloads whose zip or folder has since been deleted can no longer be accepted.
    pub(crate) fn drop_vanished(&self) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("SELECT id, path FROM sources WHERE status = 'pending'")?;
        let pending = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, path) in pending {
            if !Path::new(&path).exists() {
                self.conn.execute(
                    "UPDATE sources SET status = ?1 WHERE id = ?2",
                    params![Status::Gone.as_db(), id],
                )?;
            }
        }
        Ok(())
    }

    /// Marks the source imported and records the purchase in one transaction.
    pub(crate) fn record_import(&mut self, source: &Source, files: &[PathBuf]) -> Result<Purchase> {
        let imported_at = unix_seconds();
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE sources SET status = ?1 WHERE id = ?2",
            params![Status::Imported.as_db(), source.id],
        )?;
        tx.execute(
            "INSERT INTO purchases (source_id, store, artist, album, files, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                source.id,
                source.store.as_db(),
                source.artist,
                source.album,
                serde_json::to_string(files)?,
                imported_at,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Purchase {
            id,
            store: source.store,
            artist: source.artist.clone(),
            album: source.album.clone(),
            files: files.to_owned(),
            imported_at,
            source: source.path.clone(),
        })
    }

    /// The cached answer for `key` if it was fetched at or after `not_before`.
    pub(crate) fn cached_links(
        &self,
        key: &str,
        not_before: i64,
    ) -> Result<Option<Vec<StoreLink>>> {
        let cached: Option<String> = self
            .conn
            .query_row(
                "SELECT links FROM link_cache WHERE key = ?1 AND fetched_at >= ?2",
                params![key, not_before],
                |row| row.get(0),
            )
            .optional()?;
        Ok(cached
            .map(|links| serde_json::from_str(&links))
            .transpose()?)
    }

    pub(crate) fn cache_links(
        &self,
        key: &str,
        links: &[StoreLink],
        fetched_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO link_cache (key, links, fetched_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET links = excluded.links, fetched_at = excluded.fetched_at",
            params![key, serde_json::to_string(links)?, fetched_at],
        )?;
        Ok(())
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let supported = MIGRATIONS.len() as u32;
    if current > supported {
        return Err(Error::SchemaTooNew {
            found: current,
            supported,
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// The ledger keys on path text, so a path that is not UTF-8 cannot be remembered.
fn path_text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "path is not valid UTF-8").into()
    })
}

pub(crate) fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}
