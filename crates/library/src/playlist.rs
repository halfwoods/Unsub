use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::error::{Error, Result};
use crate::library::Library;
use crate::model::{Playlist, PlaylistKind, TrackQuery};
use crate::scan::now_unix;
use crate::smart::{
    Compile, DateUnit, LimitOrder, LimitUnit, SmartField, SmartLimit, SmartOp, SmartRule,
    SmartRules, SmartValue, smart_ids_sql,
};

const SELECT_ROW: &str = "SELECT id, name, kind, parent_id, position, rules, seed FROM playlists";
const NO_TRACKS: &str = "SELECT id FROM tracks WHERE 0";

pub(crate) struct PlaylistRow {
    pub id: i64,
    name: String,
    pub kind: PlaylistKind,
    parent_id: Option<i64>,
    position: u32,
    rules: Option<String>,
    /// Varies a random limit's pick; re-rolled whenever the rules are saved.
    pub seed: i64,
}

impl PlaylistRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            parent_id: row.get(3)?,
            position: row.get(4)?,
            rules: row.get(5)?,
            seed: row.get(6)?,
        })
    }

    pub(crate) fn smart_rules(&self) -> Result<SmartRules> {
        match &self.rules {
            Some(json) => Ok(serde_json::from_str(json)?),
            None => Ok(SmartRules::default()),
        }
    }

    /// Regular playlists and frozen smart playlists list their stored items,
    /// in order and with repeats; live and folder playlists are computed.
    pub(crate) fn holds_items(&self) -> Result<bool> {
        Ok(match self.kind {
            PlaylistKind::Regular => true,
            PlaylistKind::Smart => !self.smart_rules()?.live,
            PlaylistKind::Folder => false,
        })
    }
}

pub(crate) fn playlist_row(conn: &Connection, id: i64) -> Result<Option<PlaylistRow>> {
    Ok(conn
        .prepare_cached(&format!("{SELECT_ROW} WHERE id = ?1"))?
        .query_row([id], PlaylistRow::from_row)
        .optional()?)
}

fn require_row(conn: &Connection, id: i64) -> Result<PlaylistRow> {
    playlist_row(conn, id)?.ok_or(Error::PlaylistNotFound(id))
}

/// A `SELECT id ...` over the tracks a playlist lists, whatever its kind.
pub(crate) fn members_sql(conn: &Connection, row: &PlaylistRow, c: &mut Compile) -> Result<String> {
    if c.stack.contains(&row.id) {
        return Err(Error::PlaylistCycle);
    }
    c.stack.push(row.id);
    let sql = match row.kind {
        PlaylistKind::Smart if !row.holds_items()? => {
            smart_ids_sql(conn, &row.smart_rules()?, row.seed, c, false)?
        }
        PlaylistKind::Regular | PlaylistKind::Smart => {
            c.bind(row.id);
            "SELECT track_id AS id FROM playlist_items WHERE playlist_id = ?".to_owned()
        }
        PlaylistKind::Folder => {
            let mut parts = Vec::new();
            for child in children(conn, Some(row.id))? {
                let child = require_row(conn, child)?;
                parts.push(format!(
                    "SELECT id FROM ({})",
                    members_sql(conn, &child, c)?
                ));
            }
            if parts.is_empty() {
                NO_TRACKS.to_owned()
            } else {
                parts.join(" UNION ")
            }
        }
    };
    c.stack.pop();
    Ok(sql)
}

fn children(conn: &Connection, parent: Option<i64>) -> Result<Vec<i64>> {
    let mut stmt = conn
        .prepare_cached("SELECT id FROM playlists WHERE parent_id IS ?1 ORDER BY position, id")?;
    let ids = stmt.query_map([parent], |row| row.get(0))?;
    Ok(ids.collect::<rusqlite::Result<_>>()?)
}

/// Rewrites sibling order so positions stay 0..n with no gaps.
fn set_children(tx: &Transaction<'_>, parent: Option<i64>, ids: &[i64]) -> Result<()> {
    let mut update =
        tx.prepare_cached("UPDATE playlists SET parent_id = ?1, position = ?2 WHERE id = ?3")?;
    for (position, id) in ids.iter().enumerate() {
        update.execute(params![parent, position as u32, id])?;
    }
    Ok(())
}

fn require_folder(conn: &Connection, id: i64) -> Result<()> {
    if require_row(conn, id)?.kind != PlaylistKind::Folder {
        return Err(Error::NotAFolder(id));
    }
    Ok(())
}

fn regular_row(conn: &Connection, id: i64) -> Result<PlaylistRow> {
    let row = require_row(conn, id)?;
    if row.kind != PlaylistKind::Regular {
        return Err(Error::NotARegularPlaylist(id));
    }
    Ok(row)
}

fn load_items(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
    )?;
    let ids = stmt.query_map([id], |row| row.get(0))?;
    Ok(ids.collect::<rusqlite::Result<_>>()?)
}

/// Items are rewritten whole: renumbering in place would trip the
/// (playlist, position) key halfway through an update.
fn store_items(tx: &Transaction<'_>, id: i64, items: &[i64]) -> Result<()> {
    tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
    )?;
    for (position, track_id) in items.iter().enumerate() {
        insert.execute(params![id, position as u32, track_id])?;
    }
    Ok(())
}

/// Makes a playlist under an id nobody has had before: the one past the highest ever given out,
/// which is kept in the settings. Left to itself SQLite gives a deleted playlist's id to the next
/// one made, and an import that remembers its playlists by id would take it for the old one.
fn insert_playlist(
    conn: &Connection,
    name: &str,
    kind: PlaylistKind,
    parent_id: Option<i64>,
    position: u32,
    rules: Option<&str>,
) -> Result<i64> {
    let id: i64 = conn.query_row(
        "SELECT MAX(
            COALESCE((SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'playlist_id_floor'), 0),
            COALESCE(MAX(id), 0)
         ) + 1 FROM playlists",
        [],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO playlists (id, name, kind, parent_id, position, rules)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, name, kind, parent_id, position, rules],
    )?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('playlist_id_floor', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [id.to_string()],
    )?;
    Ok(id)
}

fn require_tracks(tx: &Transaction<'_>, track_ids: &[i64]) -> Result<()> {
    for track_id in track_ids.iter().collect::<BTreeSet<_>>() {
        let exists = tx
            .prepare_cached("SELECT 1 FROM tracks WHERE id = ?1")?
            .exists([track_id])?;
        if !exists {
            return Err(Error::TrackNotFound(*track_id));
        }
    }
    Ok(())
}

fn next_seed(previous: i64) -> i64 {
    (previous * 1_664_525 + 1_013_904_223) % (1 << 30)
}

impl Library {
    /// Parents come before their children, siblings in position order.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut by_parent: HashMap<Option<i64>, Vec<PlaylistRow>> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_ROW} ORDER BY position, id"))?;
        for row in stmt.query_map([], PlaylistRow::from_row)? {
            let row = row?;
            by_parent.entry(row.parent_id).or_default().push(row);
        }
        let mut ordered = Vec::new();
        flatten(&mut by_parent, None, &mut ordered);
        ordered.into_iter().map(|row| self.describe(row)).collect()
    }

    pub fn create_playlist(
        &mut self,
        name: &str,
        kind: PlaylistKind,
        parent_id: Option<i64>,
    ) -> Result<Playlist> {
        let tx = self.conn.transaction()?;
        if let Some(parent) = parent_id {
            require_folder(&tx, parent)?;
        }
        let position: u32 = tx.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM playlists WHERE parent_id IS ?1",
            [parent_id],
            |row| row.get(0),
        )?;
        let rules = (kind == PlaylistKind::Smart)
            .then(|| serde_json::to_string(&SmartRules::default()))
            .transpose()?;
        let id = insert_playlist(&tx, name, kind, parent_id, position, rules.as_deref())?;
        tx.commit()?;
        self.describe(require_row(&self.conn, id)?)
    }

    pub fn rename_playlist(&mut self, id: i64, name: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE playlists SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if changed == 0 {
            return Err(Error::PlaylistNotFound(id));
        }
        Ok(())
    }

    /// Foreign keys carry the deletion down to a folder's subtree and to the
    /// playlists' items.
    pub fn delete_playlist(&mut self, id: i64) -> Result<()> {
        let parent = require_row(&self.conn, id)?.parent_id;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        set_children(&tx, parent, &children(&tx, parent)?)?;
        tx.commit()?;
        Ok(())
    }

    /// `index` is the position among the new parent's children after the move.
    pub fn move_playlist(&mut self, id: i64, parent_id: Option<i64>, index: u32) -> Result<()> {
        let old_parent = require_row(&self.conn, id)?.parent_id;
        let tx = self.conn.transaction()?;
        if let Some(parent) = parent_id {
            require_folder(&tx, parent)?;
        }
        let mut ancestor = parent_id;
        while let Some(current) = ancestor {
            if current == id {
                return Err(Error::PlaylistCycle);
            }
            ancestor = tx.query_row(
                "SELECT parent_id FROM playlists WHERE id = ?1",
                [current],
                |row| row.get(0),
            )?;
        }

        let mut siblings = children(&tx, parent_id)?;
        siblings.retain(|sibling| *sibling != id);
        siblings.insert((index as usize).min(siblings.len()), id);
        set_children(&tx, parent_id, &siblings)?;
        if old_parent != parent_id {
            set_children(&tx, old_parent, &children(&tx, old_parent)?)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Repeats are allowed, as in iTunes. `at` is the index the first new
    /// track takes; past the end appends.
    pub fn add_to_playlist(&mut self, id: i64, track_ids: &[i64], at: Option<u32>) -> Result<()> {
        regular_row(&self.conn, id)?;
        let tx = self.conn.transaction()?;
        require_tracks(&tx, track_ids)?;
        let mut items = load_items(&tx, id)?;
        let index = at.map_or(items.len(), |at| (at as usize).min(items.len()));
        items.splice(index..index, track_ids.iter().copied());
        store_items(&tx, id, &items)?;
        tx.commit()?;
        Ok(())
    }

    /// Makes the playlist hold exactly `track_ids`, in that order (repeats allowed), whatever it
    /// held. All or nothing: an unknown track leaves the old contents.
    pub fn set_playlist_items(&mut self, id: i64, track_ids: &[i64]) -> Result<()> {
        regular_row(&self.conn, id)?;
        let tx = self.conn.transaction()?;
        require_tracks(&tx, track_ids)?;
        store_items(&tx, id, track_ids)?;
        tx.commit()?;
        Ok(())
    }

    /// Positions are indexes into the playlist as listed; ones past the end
    /// are ignored, like unknown ids in `remove_tracks`.
    pub fn remove_from_playlist(&mut self, id: i64, positions: &[u32]) -> Result<()> {
        regular_row(&self.conn, id)?;
        let tx = self.conn.transaction()?;
        let doomed: BTreeSet<usize> = positions.iter().map(|p| *p as usize).collect();
        let items: Vec<i64> = load_items(&tx, id)?
            .into_iter()
            .enumerate()
            .filter(|(position, _)| !doomed.contains(position))
            .map(|(_, track_id)| track_id)
            .collect();
        store_items(&tx, id, &items)?;
        tx.commit()?;
        Ok(())
    }

    /// Drag-and-drop reorder: the rows at `positions` keep their relative
    /// order and land just before the row that is at index `to` now (past the
    /// end appends).
    pub fn move_in_playlist(&mut self, id: i64, positions: &[u32], to: u32) -> Result<()> {
        regular_row(&self.conn, id)?;
        let tx = self.conn.transaction()?;
        let items = load_items(&tx, id)?;
        let moving: BTreeSet<usize> = positions
            .iter()
            .map(|p| *p as usize)
            .filter(|p| *p < items.len())
            .collect();
        let target = (to as usize).min(items.len());
        let mut reordered = Vec::with_capacity(items.len());
        let mut dropped = false;
        for (position, track_id) in items.iter().enumerate() {
            if position == target {
                reordered.extend(moving.iter().map(|p| items[*p]));
                dropped = true;
            }
            if !moving.contains(&position) {
                reordered.push(*track_id);
            }
        }
        if !dropped {
            reordered.extend(moving.iter().map(|p| items[*p]));
        }
        store_items(&tx, id, &reordered)?;
        tx.commit()?;
        Ok(())
    }

    /// Validates the rules, re-rolls the random seed, and for a non-live
    /// playlist takes the snapshot it will list.
    pub fn set_smart_rules(&mut self, id: i64, rules: &SmartRules) -> Result<()> {
        let row = require_row(&self.conn, id)?;
        if row.kind != PlaylistKind::Smart {
            return Err(Error::NotASmartPlaylist(id));
        }
        let seed = next_seed(row.seed + id);
        let mut compile = Compile::new(now_unix(), false);
        compile.stack.push(id);
        let sql = smart_ids_sql(&self.conn, rules, seed, &mut compile, true)?;
        let snapshot: Vec<i64> = if rules.live {
            Vec::new()
        } else {
            let mut stmt = self.conn.prepare(&sql)?;
            let ids =
                stmt.query_map(rusqlite::params_from_iter(compile.params), |row| row.get(0))?;
            ids.collect::<rusqlite::Result<_>>()?
        };

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE playlists SET rules = ?2, seed = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(rules)?, seed],
        )?;
        store_items(&tx, id, &snapshot)?;
        tx.commit()?;
        Ok(())
    }

    fn describe(&self, row: PlaylistRow) -> Result<Playlist> {
        let smart = (row.kind == PlaylistKind::Smart)
            .then(|| row.smart_rules())
            .transpose()?;
        let listing = TrackQuery {
            kind: None,
            playlist: Some(row.id),
            ..TrackQuery::default()
        };
        Ok(Playlist {
            id: row.id,
            name: row.name,
            kind: row.kind,
            parent_id: row.parent_id,
            position: row.position,
            smart,
            track_count: u32::try_from(self.stats(&listing)?.count).unwrap_or(u32::MAX),
        })
    }
}

fn flatten(
    by_parent: &mut HashMap<Option<i64>, Vec<PlaylistRow>>,
    parent: Option<i64>,
    out: &mut Vec<PlaylistRow>,
) {
    for row in by_parent.remove(&parent).unwrap_or_default() {
        let id = row.id;
        out.push(row);
        flatten(by_parent, Some(id), out);
    }
}

/// iTunes' stock smart playlists, written once when a database is created.
pub(crate) fn seed_defaults(conn: &Connection) -> Result<()> {
    let when = |field, weeks| {
        condition(
            field,
            SmartOp::InTheLast,
            SmartValue::Relative {
                amount: weeks,
                unit: DateUnit::Weeks,
            },
        )
    };
    let defaults = [
        (
            "90\u{2019}s Music",
            SmartRules {
                rules: vec![condition(
                    SmartField::Year,
                    SmartOp::InRange,
                    SmartValue::Range {
                        from: 1990,
                        to: 1999,
                    },
                )],
                ..SmartRules::default()
            },
        ),
        (
            "Classical Music",
            SmartRules {
                rules: vec![condition(
                    SmartField::Genre,
                    SmartOp::Is,
                    SmartValue::Text("Classical".to_owned()),
                )],
                ..SmartRules::default()
            },
        ),
        (
            "My Top Rated",
            SmartRules {
                rules: vec![condition(
                    SmartField::Rating,
                    SmartOp::GreaterThan,
                    SmartValue::Number(80),
                )],
                ..SmartRules::default()
            },
        ),
        (
            "Recently Added",
            SmartRules {
                rules: vec![when(SmartField::DateAdded, 2)],
                ..SmartRules::default()
            },
        ),
        (
            "Recently Played",
            SmartRules {
                rules: vec![when(SmartField::LastPlayed, 2)],
                ..SmartRules::default()
            },
        ),
        (
            "Top 25 Most Played",
            SmartRules {
                rules: vec![condition(
                    SmartField::Plays,
                    SmartOp::GreaterThan,
                    SmartValue::Number(0),
                )],
                limit: Some(SmartLimit {
                    amount: 25,
                    unit: LimitUnit::Items,
                    selected_by: LimitOrder::MostOftenPlayed,
                }),
                ..SmartRules::default()
            },
        ),
    ];
    for (position, (name, rules)) in defaults.iter().enumerate() {
        let rules = serde_json::to_string(rules)?;
        insert_playlist(
            conn,
            name,
            PlaylistKind::Smart,
            None,
            position as u32,
            Some(&rules),
        )?;
    }
    Ok(())
}

fn condition(field: SmartField, op: SmartOp, value: SmartValue) -> SmartRule {
    SmartRule::Condition { field, op, value }
}
