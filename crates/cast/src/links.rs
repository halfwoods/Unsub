use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::{Error, Result};

/// A link nobody has asked for or been told about in this long is gone. The session keeps the
/// links it still needs alive, so this only bounds how long a forgotten one is open.
pub const LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Bytes of randomness in a link's secret, which is its only protection.
const SECRET_BYTES: usize = 16;

struct Entry<T> {
    value: T,
    expires: Instant,
}

/// What the speakers may fetch, by secret. Each link is minted for one thing, has a secret that
/// cannot be guessed, and lapses unless used or kept alive; nothing else of the library is
/// reachable through it.
pub struct Links<T> {
    entries: Mutex<HashMap<String, Entry<T>>>,
}

impl<T> Default for Links<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
        }
    }
}

impl<T: Clone> Links<T> {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry<T>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a link to `value` and returns its secret: 32 hex digits.
    pub fn mint(&self, value: T) -> Result<String> {
        let mut bytes = [0_u8; SECRET_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|error| Error::Refused(format!("no secure randomness for a link: {error}")))?;
        let secret: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut entries = self.entries();
        let now = Instant::now();
        entries.retain(|_, entry| entry.expires > now);
        entries.insert(
            secret.clone(),
            Entry {
                value,
                expires: now + LIFETIME,
            },
        );
        Ok(secret)
    }

    /// What `secret` opens, for a request that has it; the link lives on for another [`LIFETIME`].
    pub fn open(&self, secret: &str) -> Option<T> {
        self.touch_at(secret, Instant::now())
    }

    fn touch_at(&self, secret: &str, now: Instant) -> Option<T> {
        let mut entries = self.entries();
        let entry = entries
            .get_mut(secret)
            .filter(|entry| entry.expires > now)?;
        entry.expires = now + LIFETIME;
        Some(entry.value.clone())
    }

    /// Keeps a link open for another [`LIFETIME`] without fetching it. Whether it was open.
    pub fn keep(&self, secret: &str) -> bool {
        self.open(secret).is_some()
    }

    pub fn close(&self, secret: &str) {
        self.entries().remove(secret);
    }

    /// Closes every link.
    pub fn close_all(&self) {
        self.entries().clear();
    }

    pub fn len(&self) -> usize {
        self.entries().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_long_hex_and_never_repeat() {
        let links = Links::default();
        let first = links.mint(1).unwrap();
        let second = links.mint(1).unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn a_link_opens_only_what_it_was_minted_for() {
        let links = Links::default();
        let secret = links.mint("song").unwrap();
        assert_eq!(links.open(&secret), Some("song"));
        assert_eq!(links.open("0".repeat(32).as_str()), None);
        assert_eq!(links.open(""), None);
    }

    #[test]
    fn a_closed_link_is_gone() {
        let links = Links::default();
        let (first, second) = (links.mint(1).unwrap(), links.mint(2).unwrap());
        links.close(&first);
        assert_eq!(links.open(&first), None);
        assert_eq!(links.open(&second), Some(2));
        links.close_all();
        assert!(links.is_empty());
    }

    #[test]
    fn a_link_lapses_unless_it_is_used() {
        let links = Links::default();
        let secret = links.mint(7).unwrap();
        let start = Instant::now();
        // Used just before it lapses, it lives another lifetime.
        assert_eq!(
            links.touch_at(&secret, start + LIFETIME - Duration::from_secs(1)),
            Some(7)
        );
        assert_eq!(
            links.touch_at(&secret, start + LIFETIME + Duration::from_secs(30)),
            Some(7)
        );
        // Left alone past the lifetime, it is gone for good.
        assert_eq!(links.touch_at(&secret, start + LIFETIME * 3), None);
    }

    #[test]
    fn minting_sweeps_the_links_that_have_lapsed() {
        let links = Links::default();
        let old = links.mint(1).unwrap();
        links.entries().get_mut(&old).unwrap().expires = Instant::now() - Duration::from_secs(1);
        links.mint(2).unwrap();
        assert_eq!(links.len(), 1);
    }
}
