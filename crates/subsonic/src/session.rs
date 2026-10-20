//! What the server remembers between requests: the saved play queue and who is playing what. Kept
//! in memory only; a restart forgets both, which clients cope with.

use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// A client that has not reported a song for this long is taken to have stopped.
const PLAYING_FOR: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayQueue {
    pub ids: Vec<String>,
    pub current: Option<String>,
    /// Milliseconds into the current song.
    pub position_ms: i64,
    /// Seconds since the Unix epoch.
    pub changed: i64,
    /// The name of the client that saved it.
    pub changed_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playing {
    pub song_id: String,
    pub client: String,
    pub since: Instant,
}

/// There is one account, so there is one play queue. Per-user state would key this by username.
#[derive(Default)]
pub struct Session {
    queue: Mutex<Option<PlayQueue>>,
    playing: Mutex<Vec<Playing>>,
}

impl Session {
    /// Saving nothing clears the queue.
    pub fn save_queue(&self, queue: Option<PlayQueue>) {
        *lock(&self.queue) = queue;
    }

    pub fn queue(&self) -> Option<PlayQueue> {
        lock(&self.queue).clone()
    }

    /// `client` has started `song_id`; whatever it was playing before is replaced.
    pub fn started(&self, song_id: &str, client: &str, at: Instant) {
        let mut playing = lock(&self.playing);
        playing
            .retain(|entry| entry.client != client && at.duration_since(entry.since) < PLAYING_FOR);
        playing.push(Playing {
            song_id: song_id.to_owned(),
            client: client.to_owned(),
            since: at,
        });
    }

    /// `client` has played `song_id` to the end (or far enough to count).
    pub fn finished(&self, song_id: &str, client: &str) {
        lock(&self.playing).retain(|entry| !(entry.client == client && entry.song_id == song_id));
    }

    /// Who is playing what as of `now`, the longest-running first.
    pub fn playing(&self, now: Instant) -> Vec<Playing> {
        let mut playing: Vec<Playing> = lock(&self.playing)
            .iter()
            .filter(|entry| now.saturating_duration_since(entry.since) < PLAYING_FOR)
            .cloned()
            .collect();
        playing.sort_by_key(|entry| entry.since);
        playing
    }
}

/// A panic elsewhere while the lock was held does not make the data wrong, only possibly stale.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(ids: &[&str]) -> PlayQueue {
        PlayQueue {
            ids: ids.iter().map(|&id| id.to_owned()).collect(),
            current: ids.first().map(|&id| id.to_owned()),
            position_ms: 1234,
            changed: 1_700_000_000,
            changed_by: "tests".to_owned(),
        }
    }

    #[test]
    fn the_queue_is_replaced_and_cleared() {
        let session = Session::default();
        assert_eq!(session.queue(), None);
        session.save_queue(Some(queue(&["a", "b"])));
        assert_eq!(session.queue(), Some(queue(&["a", "b"])));
        session.save_queue(Some(queue(&["c"])));
        assert_eq!(session.queue().unwrap().ids, ["c"]);
        session.save_queue(None);
        assert_eq!(session.queue(), None);
    }

    #[test]
    fn a_client_plays_one_song_at_a_time() {
        let session = Session::default();
        let now = Instant::now();
        session.started("s1", "phone", now);
        session.started("s2", "tablet", now);
        session.started("s3", "phone", now);
        let songs: Vec<_> = session
            .playing(now)
            .into_iter()
            .map(|p| (p.song_id, p.client))
            .collect();
        assert_eq!(
            songs,
            [
                ("s2".to_owned(), "tablet".to_owned()),
                ("s3".to_owned(), "phone".to_owned())
            ]
        );
    }

    #[test]
    fn finishing_removes_only_that_song_for_that_client() {
        let session = Session::default();
        let now = Instant::now();
        session.started("s1", "phone", now);
        session.finished("other", "phone");
        session.finished("s1", "tablet");
        assert_eq!(session.playing(now).len(), 1);
        session.finished("s1", "phone");
        assert!(session.playing(now).is_empty());
    }

    #[test]
    fn a_client_that_goes_quiet_stops_counting() {
        let session = Session::default();
        let start = Instant::now();
        session.started("s1", "phone", start);
        assert_eq!(
            session
                .playing(start + PLAYING_FOR - Duration::from_secs(1))
                .len(),
            1
        );
        assert!(session.playing(start + PLAYING_FOR).is_empty());
        // And a later report from anyone prunes it for good.
        session.started("s2", "tablet", start + PLAYING_FOR);
        assert_eq!(session.playing(start).len(), 1);
    }
}
