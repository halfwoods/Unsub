use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub author: String,
    pub description: String,
    pub artwork_url: Option<String>,
    pub link: Option<String>,
    /// Unix seconds of the last successful check, including `304 Not Modified` answers.
    pub last_refreshed: Option<i64>,
    pub unplayed_count: u32,
    pub settings: FeedSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedSettings {
    /// Download the newest N episodes automatically; 0 turns the rule off.
    pub auto_download_latest: u32,
    pub keep: KeepRule,
}

/// Which downloaded files `Podcasts::apply_cleanup` keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepRule {
    #[default]
    All,
    /// Files of the newest N episodes of the feed, played or not.
    LastN(u32),
    /// Files of episodes that have not been played yet.
    Unplayed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub title: String,
    pub description: String,
    /// Unix seconds.
    pub published: Option<i64>,
    pub duration_ms: Option<u64>,
    pub enclosure_url: String,
    pub enclosure_type: Option<String>,
    pub enclosure_length: Option<u64>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub explicit: bool,
    pub artwork_url: Option<String>,
    pub played: bool,
    pub position_ms: u64,
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSummary {
    pub new_episodes: u32,
    /// Existing episodes whose title, notes, enclosure or other feed-supplied details changed.
    pub updated_episodes: u32,
    pub not_modified: bool,
}
