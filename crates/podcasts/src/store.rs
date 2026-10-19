use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::error::{Error, Result};
use crate::feed::{ParsedEpisode, ParsedFeed};
use crate::http::Validators;
use crate::model::{Episode, Feed, FeedSettings, KeepRule, RefreshSummary};

/// Appended to, never edited: `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE feeds (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL,
        author TEXT NOT NULL DEFAULT '',
        description TEXT NOT NULL DEFAULT '',
        artwork_url TEXT,
        link TEXT,
        etag TEXT,
        last_modified TEXT,
        last_refreshed INTEGER,
        auto_download_latest INTEGER NOT NULL DEFAULT 0,
        keep_rule TEXT NOT NULL DEFAULT 'all',
        keep_count INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE episodes (
        id INTEGER PRIMARY KEY,
        feed_id INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
        guid TEXT NOT NULL,
        title TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        published INTEGER,
        -- First-seen time, so undated episodes still sort by when they appeared.
        added INTEGER NOT NULL,
        duration_ms INTEGER,
        enclosure_url TEXT NOT NULL,
        enclosure_type TEXT,
        enclosure_length INTEGER,
        season INTEGER,
        episode INTEGER,
        explicit INTEGER NOT NULL DEFAULT 0,
        artwork_url TEXT,
        played INTEGER NOT NULL DEFAULT 0,
        position_ms INTEGER NOT NULL DEFAULT 0,
        file TEXT,
        -- Set when cleanup deleted the file, so auto-download does not fetch it again.
        cleaned INTEGER NOT NULL DEFAULT 0,
        UNIQUE (feed_id, guid)
    );
    CREATE INDEX episodes_by_feed ON episodes (feed_id);
"];

const FEED_COLUMNS: &str = "f.id, f.url, f.title, f.author, f.description, f.artwork_url, f.link,
    f.last_refreshed,
    (SELECT COUNT(*) FROM episodes e WHERE e.feed_id = f.id AND e.played = 0),
    f.auto_download_latest, f.keep_rule, f.keep_count";

const EPISODE_COLUMNS: &str = "id, feed_id, guid, title, description, published, duration_ms,
    enclosure_url, enclosure_type, enclosure_length, season, episode, explicit, artwork_url,
    played, position_ms, file";

const NEWEST_FIRST: &str = "COALESCE(published, added) DESC, id DESC";

pub(crate) struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let mut conn = Connection::open(path)?;
        // WAL plus a busy timeout lets the app open a second handle for the download worker
        // while the UI handle keeps reading.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    pub fn feeds(&self) -> Result<Vec<Feed>> {
        let sql =
            format!("SELECT {FEED_COLUMNS} FROM feeds f ORDER BY f.title COLLATE NOCASE, f.id");
        let mut statement = self.conn.prepare(&sql)?;
        let feeds = statement
            .query_map([], feed_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(feeds)
    }

    pub fn feed(&self, feed_id: i64) -> Result<Feed> {
        let sql = format!("SELECT {FEED_COLUMNS} FROM feeds f WHERE f.id = ?1");
        self.conn
            .query_row(&sql, [feed_id], feed_from_row)
            .optional()?
            .ok_or(Error::FeedNotFound(feed_id))
    }

    pub fn fetch_target(&self, feed_id: i64) -> Result<(String, Validators)> {
        self.conn
            .query_row(
                "SELECT url, etag, last_modified FROM feeds WHERE id = ?1",
                [feed_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        Validators {
                            etag: row.get(1)?,
                            last_modified: row.get(2)?,
                        },
                    ))
                },
            )
            .optional()?
            .ok_or(Error::FeedNotFound(feed_id))
    }

    /// Stores a fetched feed: updates `existing`, else the feed with the same URL, else inserts.
    pub fn save_feed(
        &mut self,
        existing: Option<i64>,
        url: &str,
        parsed: &ParsedFeed,
        validators: &Validators,
    ) -> Result<(i64, RefreshSummary)> {
        let now = unix_now();
        let tx = self.conn.transaction()?;
        let known = match existing {
            Some(id) => Some(id),
            None => tx
                .query_row("SELECT id FROM feeds WHERE url = ?1", [url], |row| {
                    row.get(0)
                })
                .optional()?,
        };
        let feed_id = match known {
            Some(id) => {
                update_feed(&tx, id, parsed, validators, now)?;
                id
            }
            None => insert_feed(&tx, url, parsed, validators, now)?,
        };
        let summary = upsert_episodes(&tx, feed_id, &parsed.episodes, now)?;
        tx.commit()?;
        Ok((feed_id, summary))
    }

    pub fn touch_refreshed(&self, feed_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE feeds SET last_refreshed = ?2 WHERE id = ?1",
            params![feed_id, unix_now()],
        )?;
        Ok(())
    }

    pub fn set_settings(&self, feed_id: i64, settings: &FeedSettings) -> Result<()> {
        let (rule, count) = match settings.keep {
            KeepRule::All => ("all", 0),
            KeepRule::LastN(count) => ("last", count),
            KeepRule::Unplayed => ("unplayed", 0),
        };
        let changed = self.conn.execute(
            "UPDATE feeds SET auto_download_latest = ?2, keep_rule = ?3, keep_count = ?4
             WHERE id = ?1",
            params![feed_id, settings.auto_download_latest, rule, count],
        )?;
        require_row(changed, Error::FeedNotFound(feed_id))
    }

    /// Deletes the feed and, through the foreign key, its episodes.
    pub fn delete_feed(&self, feed_id: i64) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM feeds WHERE id = ?1", [feed_id])?;
        require_row(changed, Error::FeedNotFound(feed_id))
    }

    pub fn episodes(&self, feed_id: i64) -> Result<Vec<Episode>> {
        let sql = format!(
            "SELECT {EPISODE_COLUMNS} FROM episodes WHERE feed_id = ?1 ORDER BY {NEWEST_FIRST}"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let episodes = statement
            .query_map([feed_id], episode_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(episodes)
    }

    pub fn episode(&self, episode_id: i64) -> Result<Episode> {
        let sql = format!("SELECT {EPISODE_COLUMNS} FROM episodes WHERE id = ?1");
        self.conn
            .query_row(&sql, [episode_id], episode_from_row)
            .optional()?
            .ok_or(Error::EpisodeNotFound(episode_id))
    }

    pub fn downloaded_files(&self, feed_id: i64) -> Result<Vec<PathBuf>> {
        let mut statement = self
            .conn
            .prepare("SELECT file FROM episodes WHERE feed_id = ?1 AND file IS NOT NULL")?;
        let files = statement
            .query_map([feed_id], |row| row.get::<_, String>(0).map(PathBuf::from))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(files)
    }

    pub fn set_file(&self, episode_id: i64, file: &Path) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE episodes SET file = ?2, cleaned = 0 WHERE id = ?1",
            params![episode_id, file.to_string_lossy()],
        )?;
        require_row(changed, Error::EpisodeNotFound(episode_id))
    }

    pub fn mark_cleaned(&self, episode_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE episodes SET file = NULL, cleaned = 1 WHERE id = ?1",
            [episode_id],
        )?;
        Ok(())
    }

    pub fn set_played(&self, episode_id: i64, played: bool) -> Result<()> {
        // A finished episode should restart from the top if it is played again.
        let changed = self.conn.execute(
            "UPDATE episodes SET played = ?2, position_ms = CASE WHEN ?2 THEN 0 ELSE position_ms END
             WHERE id = ?1",
            params![episode_id, played],
        )?;
        require_row(changed, Error::EpisodeNotFound(episode_id))
    }

    pub fn set_position(&self, episode_id: i64, position_ms: u64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE episodes SET position_ms = ?2 WHERE id = ?1",
            params![episode_id, position_ms],
        )?;
        require_row(changed, Error::EpisodeNotFound(episode_id))
    }

    /// Unplayed, undownloaded episodes inside each feed's newest-N window.
    pub fn pending_auto_downloads(&self) -> Result<Vec<i64>> {
        let mut pending = Vec::new();
        let mut statement = self.conn.prepare(&format!(
            "SELECT id FROM (
                 SELECT id, file, played, cleaned FROM episodes
                 WHERE feed_id = ?1 ORDER BY {NEWEST_FIRST} LIMIT ?2
             ) WHERE file IS NULL AND played = 0 AND cleaned = 0"
        ))?;
        for feed in self.feeds()? {
            let window = feed.settings.auto_download_latest;
            if window == 0 {
                continue;
            }
            let ids = statement.query_map(params![feed.id, window], |row| row.get::<_, i64>(0))?;
            for id in ids {
                pending.push(id?);
            }
        }
        Ok(pending)
    }

    /// Downloaded episodes that fall outside their feed's keep rule.
    pub fn cleanup_candidates(&self) -> Result<Vec<(i64, PathBuf)>> {
        let mut candidates = Vec::new();
        for feed in self.feeds()? {
            match feed.settings.keep {
                KeepRule::All => {}
                KeepRule::Unplayed => candidates.extend(self.files_where(
                    "SELECT id, file FROM episodes
                     WHERE feed_id = ?1 AND played = 1 AND file IS NOT NULL",
                    params![feed.id],
                )?),
                KeepRule::LastN(count) => candidates.extend(self.files_where(
                    &format!(
                        "SELECT id, file FROM (
                             SELECT id, file FROM episodes
                             WHERE feed_id = ?1 ORDER BY {NEWEST_FIRST} LIMIT -1 OFFSET ?2
                         ) WHERE file IS NOT NULL"
                    ),
                    params![feed.id, count],
                )?),
            }
        }
        Ok(candidates)
    }

    fn files_where(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<(i64, PathBuf)>> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement
            .query_map(params, |row| {
                Ok((row.get(0)?, PathBuf::from(row.get::<_, String>(1)?)))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: usize = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > MIGRATIONS.len() {
        return Err(Error::UnsupportedSchema(version));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index + 1)?;
        tx.commit()?;
    }
    Ok(())
}

fn insert_feed(
    tx: &Transaction,
    url: &str,
    parsed: &ParsedFeed,
    validators: &Validators,
    now: i64,
) -> Result<i64> {
    // A feed without a title is still listable by its address.
    let title = if parsed.title.is_empty() {
        url
    } else {
        parsed.title.as_str()
    };
    tx.execute(
        "INSERT INTO feeds (url, title, author, description, artwork_url, link, etag,
                            last_modified, last_refreshed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            url,
            title,
            parsed.author,
            parsed.description,
            parsed.artwork_url,
            parsed.link,
            validators.etag,
            validators.last_modified,
            now,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn update_feed(
    tx: &Transaction,
    feed_id: i64,
    parsed: &ParsedFeed,
    validators: &Validators,
    now: i64,
) -> Result<()> {
    let changed = tx.execute(
        "UPDATE feeds SET title = COALESCE(NULLIF(?2, ''), title), author = ?3, description = ?4,
                          artwork_url = ?5, link = ?6, etag = ?7, last_modified = ?8,
                          last_refreshed = ?9
         WHERE id = ?1",
        params![
            feed_id,
            parsed.title,
            parsed.author,
            parsed.description,
            parsed.artwork_url,
            parsed.link,
            validators.etag,
            validators.last_modified,
            now,
        ],
    )?;
    require_row(changed, Error::FeedNotFound(feed_id))
}

fn upsert_episodes(
    tx: &Transaction,
    feed_id: i64,
    episodes: &[ParsedEpisode],
    now: i64,
) -> Result<RefreshSummary> {
    let mut summary = RefreshSummary::default();
    // Feeds list newest first; inserting oldest first keeps row ids increasing with recency,
    // which is the tie-break for undated episodes.
    for episode in episodes.iter().rev() {
        match find_episode(tx, feed_id, episode)? {
            None => {
                insert_episode(tx, feed_id, episode, now)?;
                summary.new_episodes += 1;
            }
            Some((id, stored)) if stored != *episode => {
                update_episode(tx, id, episode)?;
                summary.updated_episodes += 1;
            }
            Some(_) => {}
        }
    }
    Ok(summary)
}

/// Matches by guid first; the enclosure URL catches feeds that regenerate their guids.
fn find_episode(
    tx: &Transaction,
    feed_id: i64,
    episode: &ParsedEpisode,
) -> Result<Option<(i64, ParsedEpisode)>> {
    Ok(tx
        .query_row(
            "SELECT id, guid, title, description, published, duration_ms, enclosure_url,
                    enclosure_type, enclosure_length, season, episode, explicit, artwork_url
             FROM episodes
             WHERE feed_id = ?1 AND (guid = ?2 OR enclosure_url = ?3)
             ORDER BY guid = ?2 DESC LIMIT 1",
            params![feed_id, episode.guid, episode.enclosure_url],
            |row| {
                Ok((
                    row.get(0)?,
                    ParsedEpisode {
                        guid: row.get(1)?,
                        title: row.get(2)?,
                        description: row.get(3)?,
                        published: row.get(4)?,
                        duration_ms: row.get(5)?,
                        enclosure_url: row.get(6)?,
                        enclosure_type: row.get(7)?,
                        enclosure_length: row.get(8)?,
                        season: row.get(9)?,
                        episode: row.get(10)?,
                        explicit: row.get(11)?,
                        artwork_url: row.get(12)?,
                    },
                ))
            },
        )
        .optional()?)
}

fn insert_episode(tx: &Transaction, feed_id: i64, episode: &ParsedEpisode, now: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO episodes (feed_id, guid, title, description, published, added, duration_ms,
                               enclosure_url, enclosure_type, enclosure_length, season, episode,
                               explicit, artwork_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            feed_id,
            episode.guid,
            episode.title,
            episode.description,
            episode.published,
            now,
            episode.duration_ms,
            episode.enclosure_url,
            episode.enclosure_type,
            episode.enclosure_length,
            episode.season,
            episode.episode,
            episode.explicit,
            episode.artwork_url,
        ],
    )?;
    Ok(())
}

/// Leaves played state, position and the downloaded file alone: those belong to the listener.
fn update_episode(tx: &Transaction, id: i64, episode: &ParsedEpisode) -> Result<()> {
    tx.execute(
        "UPDATE episodes SET guid = ?2, title = ?3, description = ?4, published = ?5,
                             duration_ms = ?6, enclosure_url = ?7, enclosure_type = ?8,
                             enclosure_length = ?9, season = ?10, episode = ?11, explicit = ?12,
                             artwork_url = ?13
         WHERE id = ?1",
        params![
            id,
            episode.guid,
            episode.title,
            episode.description,
            episode.published,
            episode.duration_ms,
            episode.enclosure_url,
            episode.enclosure_type,
            episode.enclosure_length,
            episode.season,
            episode.episode,
            episode.explicit,
            episode.artwork_url,
        ],
    )?;
    Ok(())
}

fn feed_from_row(row: &Row) -> rusqlite::Result<Feed> {
    let keep_count: u32 = row.get(11)?;
    let keep = match row.get::<_, String>(10)?.as_str() {
        "last" => KeepRule::LastN(keep_count),
        "unplayed" => KeepRule::Unplayed,
        _ => KeepRule::All,
    };
    Ok(Feed {
        id: row.get(0)?,
        url: row.get(1)?,
        title: row.get(2)?,
        author: row.get(3)?,
        description: row.get(4)?,
        artwork_url: row.get(5)?,
        link: row.get(6)?,
        last_refreshed: row.get(7)?,
        unplayed_count: row.get(8)?,
        settings: FeedSettings {
            auto_download_latest: row.get(9)?,
            keep,
        },
    })
}

fn episode_from_row(row: &Row) -> rusqlite::Result<Episode> {
    Ok(Episode {
        id: row.get(0)?,
        feed_id: row.get(1)?,
        guid: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        published: row.get(5)?,
        duration_ms: row.get(6)?,
        enclosure_url: row.get(7)?,
        enclosure_type: row.get(8)?,
        enclosure_length: row.get(9)?,
        season: row.get(10)?,
        episode: row.get(11)?,
        explicit: row.get(12)?,
        artwork_url: row.get(13)?,
        played: row.get(14)?,
        position_ms: row.get(15)?,
        file: row.get::<_, Option<String>>(16)?.map(PathBuf::from),
    })
}

fn require_row(changed: usize, missing: Error) -> Result<()> {
    if changed == 0 { Err(missing) } else { Ok(()) }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}
