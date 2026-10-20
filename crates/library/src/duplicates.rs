use std::collections::BTreeMap;

use crate::error::Result;
use crate::library::Library;
use crate::model::RADIO;
use crate::normalize::{artist_key, title_key};

/// Within this many milliseconds of each other, two lengths are the same recording.
const SAME_LENGTH_MS: u64 = 2_000;

struct Row {
    id: i64,
    album: Option<String>,
    duration_ms: u64,
}

impl Library {
    /// Groups of tracks that look like the same song twice. Tracks match on
    /// title and primary artist as the matcher reads them (so a remaster
    /// still matches, a live version does not). `exact` is iTunes' "Show Exact
    /// Duplicates": the album must match too, and lengths within two seconds.
    /// Groups are ordered by their lowest id, and ids within one ascend.
    pub fn duplicates(&self, exact: bool) -> Result<Vec<Vec<i64>>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, title, artist, album, duration_ms FROM tracks
                 WHERE kind <> {RADIO} ORDER BY id"
        ))?;
        let mut rows = stmt.query([])?;
        let mut buckets: BTreeMap<(String, String), Vec<Row>> = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let title: String = row.get(1)?;
            let artist: String = row.get(2)?;
            // Without a title and an artist there is nothing to compare.
            if let (Some(title), Some(artist)) = (title_key(&title), artist_key(&artist)) {
                let album: String = row.get(3)?;
                buckets.entry((title, artist)).or_default().push(Row {
                    id: row.get(0)?,
                    album: title_key(&album),
                    duration_ms: row.get(4)?,
                });
            }
        }

        let mut groups: Vec<Vec<i64>> = buckets
            .into_values()
            .flat_map(|bucket| {
                if exact {
                    exact_groups(bucket)
                } else {
                    vec![bucket]
                }
            })
            .filter(|group| group.len() > 1)
            .map(|group| group.iter().map(|row| row.id).collect())
            .collect();
        groups.sort_by_key(|group| group[0]);
        Ok(groups)
    }
}

/// Splits a bucket by album, then by length: each group starts at its shortest
/// track and takes those within the tolerance of it, so no two members are
/// further apart than that.
fn exact_groups(bucket: Vec<Row>) -> Vec<Vec<Row>> {
    let mut by_album: BTreeMap<Option<String>, Vec<Row>> = BTreeMap::new();
    for row in bucket {
        by_album.entry(row.album.clone()).or_default().push(row);
    }
    let mut groups = Vec::new();
    for mut rows in by_album.into_values() {
        rows.sort_by_key(|row| (row.duration_ms, row.id));
        let mut current: Vec<Row> = Vec::new();
        for row in rows {
            if current
                .first()
                .is_some_and(|first| row.duration_ms - first.duration_ms > SAME_LENGTH_MS)
            {
                groups.push(std::mem::take(&mut current));
            }
            current.push(row);
        }
        groups.push(current);
    }
    for group in &mut groups {
        group.sort_by_key(|row| row.id);
    }
    groups
}
