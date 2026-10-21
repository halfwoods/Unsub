//! iTunes-style podcast subscriptions: feeds, episodes, downloads and listening state, kept in
//! their own SQLite database apart from the music library.

mod directory;
mod download;
mod error;
mod feed;
mod http;
mod model;
mod opml;
pub mod podcasts_app;
mod stall;
mod store;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use store::Store;

pub use directory::{
    CATEGORIES, Category, Directory, Listing, Listings, Source, default_directory,
};
pub use error::{Error, Result};
pub use model::{Episode, Feed, FeedSettings, KeepRule, RefreshSummary, Subscribe};

/// A feed that has been fetched and parsed but not stored. Fetching needs no store, so the app
/// can do it without holding up whatever else is using the store.
pub struct FetchedFeed {
    /// The address the feed was asked for, before any redirect.
    requested_url: String,
    final_url: String,
    parsed: feed::ParsedFeed,
    validators: http::Validators,
}

fn fetch_new(agent: &ureq::Agent, feed_url: &str) -> Result<FetchedFeed> {
    let url = http::normalize_feed_url(feed_url)?;
    let response =
        http::fetch_feed(agent, &url, &http::Validators::default())?.ok_or_else(|| {
            Error::Status {
                status: 304,
                url: url.clone(),
            }
        })?;
    Ok(FetchedFeed {
        requested_url: url,
        final_url: response.final_url,
        parsed: feed::parse(&response.body)?,
        validators: response.validators,
    })
}

/// What a stored feed needs to be asked for its changes. It is read from the store, so the
/// request itself can be made without one.
pub struct RefreshRequest {
    feed_id: i64,
    url: String,
    validators: http::Validators,
}

impl RefreshRequest {
    /// The network half of [`Podcasts::refresh`]. Pass the result to [`Podcasts::save_update`].
    pub fn fetch(self) -> Result<FetchedUpdate> {
        self.fetch_with(&http::agent(http::DEFAULT_STALL_TIMEOUT))
    }

    fn fetch_with(self, agent: &ureq::Agent) -> Result<FetchedUpdate> {
        let changed = match http::fetch_feed(agent, &self.url, &self.validators)? {
            Some(response) => Some((feed::parse(&response.body)?, response.validators)),
            None => None,
        };
        Ok(FetchedUpdate {
            feed_id: self.feed_id,
            url: self.url,
            changed,
        })
    }
}

/// A feed's answer to a refresh, not yet stored.
pub struct FetchedUpdate {
    feed_id: i64,
    url: String,
    /// `None` when the server said nothing had changed.
    changed: Option<(feed::ParsedFeed, http::Validators)>,
}

pub struct Podcasts {
    store: Store,
    download_dir: PathBuf,
    agent: ureq::Agent,
}

impl Podcasts {
    pub fn open(db_path: &Path, download_dir: &Path) -> Result<Podcasts> {
        fs::create_dir_all(download_dir)?;
        Ok(Podcasts {
            store: Store::open(db_path)?,
            download_dir: download_dir.to_path_buf(),
            agent: http::agent(http::DEFAULT_STALL_TIMEOUT),
        })
    }

    /// How long a request may go without receiving any bytes before it fails; 30 seconds by
    /// default. A download that fails this way (`Error::Stalled`) keeps its partial file for
    /// resuming. Total transfer time is never limited.
    pub fn set_stall_timeout(&mut self, timeout: Duration) {
        self.agent = http::agent(timeout);
    }

    /// The network half of [`Podcasts::subscribe`], for callers that must not hold a store while
    /// waiting on a slow server. Pass the result to [`Podcasts::save_fetched`].
    pub fn fetch(feed_url: &str) -> Result<FetchedFeed> {
        fetch_new(&http::agent(http::DEFAULT_STALL_TIMEOUT), feed_url)
    }

    /// Fetches and stores the feed under its post-redirect URL. Subscribing to a feed that is
    /// already stored refreshes it instead of adding a duplicate.
    pub fn subscribe(&mut self, feed_url: &str) -> Result<Feed> {
        let fetched = fetch_new(&self.agent, feed_url)?;
        self.save_fetched(fetched, Subscribe::One)
    }

    /// Stores the feed under its post-redirect URL and remembers the address it was asked for,
    /// which directories list instead of it, so the show can be recognised as followed.
    pub fn save_fetched(&mut self, fetched: FetchedFeed, subscribe: Subscribe) -> Result<Feed> {
        let (feed_id, _) = self.store.save_feed(
            None,
            &fetched.final_url,
            &fetched.parsed,
            &fetched.validators,
            subscribe.auto_download_latest(),
        )?;
        if fetched.requested_url != fetched.final_url {
            self.store.add_alias(feed_id, &fetched.requested_url)?;
        }
        self.store.feed(feed_id)
    }

    /// `delete_files` also removes downloaded episodes and half-finished downloads from disk.
    pub fn unsubscribe(&mut self, feed_id: i64, delete_files: bool) -> Result<()> {
        if delete_files {
            let feed = self.store.feed(feed_id)?;
            for file in self.store.downloaded_files(feed_id)? {
                remove_file(&file)?;
            }
            let dir = download::feed_dir(&self.download_dir, &feed.title, feed_id);
            for episode in self.store.episodes(feed_id)? {
                download::discard_partial(&dir, episode.id)?;
            }
            // Only succeeds when empty, which is the only case where removing it is right.
            let _ = fs::remove_dir(&dir);
        }
        self.store.delete_feed(feed_id)
    }

    pub fn refresh(&mut self, feed_id: i64) -> Result<RefreshSummary> {
        let update = self.refresh_request(feed_id)?.fetch_with(&self.agent)?;
        self.save_update(update)
    }

    pub fn refresh_request(&self, feed_id: i64) -> Result<RefreshRequest> {
        let (url, validators) = self.store.fetch_target(feed_id)?;
        Ok(RefreshRequest {
            feed_id,
            url,
            validators,
        })
    }

    /// Fails with `FeedNotFound` if the feed was unsubscribed since the request was made, even
    /// if another show has been given its id since.
    pub fn save_update(&mut self, update: FetchedUpdate) -> Result<RefreshSummary> {
        let (url, _) = self.store.fetch_target(update.feed_id)?;
        if url != update.url {
            return Err(Error::FeedNotFound(update.feed_id));
        }
        let Some((parsed, validators)) = update.changed else {
            self.store.touch_refreshed(update.feed_id)?;
            return Ok(RefreshSummary {
                not_modified: true,
                ..RefreshSummary::default()
            });
        };
        // The feed is stored, so the setting for a new one goes unused.
        let (_, summary) =
            self.store
                .save_feed(Some(update.feed_id), &update.url, &parsed, &validators, 0)?;
        Ok(summary)
    }

    /// One failing feed does not stop the others; each result is reported per feed id.
    pub fn refresh_all(
        &mut self,
    ) -> Result<Vec<(i64, std::result::Result<RefreshSummary, String>)>> {
        let mut results = Vec::new();
        for feed in self.store.feeds()? {
            let result = self.refresh(feed.id).map_err(|err| err.to_string());
            results.push((feed.id, result));
        }
        Ok(results)
    }

    /// Sorted by title, case-insensitively.
    pub fn feeds(&self) -> Result<Vec<Feed>> {
        self.store.feeds()
    }

    /// Newest first.
    pub fn episodes(&self, feed_id: i64) -> Result<Vec<Episode>> {
        self.store.episodes(feed_id)
    }

    /// Returns the existing file when the episode is already downloaded. A cancelled or failed
    /// download keeps its partial file, and calling this again resumes it.
    pub fn download(
        &mut self,
        episode_id: i64,
        progress: &mut dyn FnMut(u64, Option<u64>),
        cancel: &AtomicBool,
    ) -> Result<PathBuf> {
        let episode = self.store.episode(episode_id)?;
        if let Some(file) = &episode.file
            && file.exists()
        {
            return Ok(file.clone());
        }
        let feed = self.store.feed(episode.feed_id)?;
        let file = download::fetch(
            &self.agent,
            &self.download_dir,
            &feed.title,
            &episode,
            progress,
            cancel,
        )?;
        self.store.set_file(episode_id, &file)?;
        Ok(file)
    }

    /// Episodes the per-feed "download the latest N" rules want that are not on disk yet.
    /// Played episodes and ones removed by `apply_cleanup` are not requested again.
    pub fn pending_auto_downloads(&self) -> Result<Vec<i64>> {
        self.store.pending_auto_downloads()
    }

    /// Deletes downloaded files that fall outside each feed's keep rule and returns them.
    pub fn apply_cleanup(&mut self) -> Result<Vec<PathBuf>> {
        let mut deleted = Vec::new();
        for (episode_id, file) in self.store.cleanup_candidates()? {
            if remove_file(&file)? {
                deleted.push(file);
            }
            self.store.mark_cleaned(episode_id)?;
        }
        Ok(deleted)
    }

    /// Marking an episode played also rewinds its resume position.
    pub fn set_played(&mut self, episode_id: i64, played: bool) -> Result<()> {
        self.store.set_played(episode_id, played)
    }

    pub fn set_position(&mut self, episode_id: i64, position_ms: u64) -> Result<()> {
        self.store.set_position(episode_id, position_ms)
    }

    pub fn set_settings(&mut self, feed_id: i64, settings: FeedSettings) -> Result<()> {
        self.store.set_settings(feed_id, &settings)
    }

    /// Subscribes to every feed in the document, one result per outline that has a feed URL. The
    /// feeds start with nothing downloading, as a bulk import should.
    pub fn import_opml(&mut self, opml: &str) -> Result<Vec<std::result::Result<Feed, String>>> {
        let urls = opml::feed_urls(opml)?;
        Ok(urls
            .iter()
            .map(|url| {
                fetch_new(&self.agent, url)
                    .and_then(|fetched| self.save_fetched(fetched, Subscribe::Bulk))
                    .map_err(|err| err.to_string())
            })
            .collect())
    }

    pub fn export_opml(&self) -> Result<String> {
        Ok(opml::write(&self.store.feeds()?))
    }
}

/// Returns whether a file was actually deleted; a file that is already gone is not an error.
fn remove_file(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}
