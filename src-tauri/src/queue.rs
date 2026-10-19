use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

/// The "play context" iTunes snapshots when you start playing from a list: later edits to the
/// list don't reorder what's already queued.
#[derive(Debug)]
pub struct PlayQueue {
    source: Vec<i64>,
    order: Vec<i64>,
    position: usize,
    shuffle: bool,
    repeat: RepeatMode,
}

impl PlayQueue {
    pub fn new(track_ids: Vec<i64>, start: usize, shuffle: bool, repeat: RepeatMode) -> Option<Self> {
        if start >= track_ids.len() {
            return None;
        }
        let mut queue = Self {
            order: track_ids.clone(),
            source: track_ids,
            position: start,
            shuffle: false,
            repeat,
        };
        queue.set_shuffle(shuffle);
        Some(queue)
    }

    pub fn current(&self) -> i64 {
        self.order[self.position]
    }

    /// What plays when the current track ends on its own.
    pub fn peek_next(&self) -> Option<i64> {
        match self.repeat {
            RepeatMode::One => Some(self.current()),
            _ => self.index_after(self.position).map(|i| self.order[i]),
        }
    }

    /// Moves to `track_id` if it's the natural successor; called when the engine reports a
    /// gapless transition. Returns false if the engine played something we didn't expect.
    pub fn sync_to(&mut self, track_id: i64) -> bool {
        if self.current() == track_id {
            return true;
        }
        match self.index_after(self.position) {
            Some(next) if self.order[next] == track_id => {
                self.position = next;
                true
            }
            _ => false,
        }
    }

    /// The Next button: repeat-one doesn't trap a manual skip, as in iTunes.
    pub fn skip_next(&mut self) -> Option<i64> {
        let next = self.index_after(self.position)?;
        self.position = next;
        Some(self.current())
    }

    pub fn skip_previous(&mut self) -> Option<i64> {
        let previous = match self.position {
            0 if self.repeat == RepeatMode::All => self.order.len() - 1,
            0 => return None,
            p => p - 1,
        };
        self.position = previous;
        Some(self.current())
    }

    /// Reshuffles around the current track so turning shuffle on never interrupts playback.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        let current = self.current();
        self.shuffle = shuffle;
        if shuffle {
            let mut rest: Vec<i64> = self.source.iter().copied().filter(|&id| id != current).collect();
            fastrand::shuffle(&mut rest);
            self.order = std::iter::once(current).chain(rest).collect();
            self.position = 0;
        } else {
            self.order = self.source.clone();
            self.position = self.source.iter().position(|&id| id == current).unwrap_or(0);
        }
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    fn index_after(&self, index: usize) -> Option<usize> {
        if index + 1 < self.order.len() {
            Some(index + 1)
        } else if self.repeat != RepeatMode::Off {
            Some(0)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(start: usize, repeat: RepeatMode) -> PlayQueue {
        PlayQueue::new(vec![10, 20, 30], start, false, repeat).expect("valid start")
    }

    #[test]
    fn rejects_out_of_range_start() {
        assert!(PlayQueue::new(vec![1], 1, false, RepeatMode::Off).is_none());
    }

    #[test]
    fn plays_in_order_and_stops_at_the_end() {
        let mut q = queue(1, RepeatMode::Off);
        assert_eq!(q.peek_next(), Some(30));
        assert!(q.sync_to(30));
        assert_eq!(q.peek_next(), None);
        assert_eq!(q.skip_next(), None);
    }

    #[test]
    fn repeat_all_wraps_both_ways() {
        let mut q = queue(2, RepeatMode::All);
        assert_eq!(q.peek_next(), Some(10));
        assert_eq!(q.skip_next(), Some(10));
        assert_eq!(q.skip_previous(), Some(30));
    }

    #[test]
    fn repeat_one_loops_but_next_still_skips() {
        let mut q = queue(0, RepeatMode::One);
        assert_eq!(q.peek_next(), Some(10));
        assert!(q.sync_to(10));
        assert_eq!(q.skip_next(), Some(20));
    }

    #[test]
    fn previous_at_start_without_repeat_is_none() {
        let mut q = queue(0, RepeatMode::Off);
        assert_eq!(q.skip_previous(), None);
    }

    #[test]
    fn sync_rejects_unexpected_tracks() {
        let mut q = queue(0, RepeatMode::Off);
        assert!(!q.sync_to(30));
        assert_eq!(q.current(), 10);
    }

    #[test]
    fn shuffle_keeps_current_first_and_every_track_once() {
        let mut q = PlayQueue::new((1..=50).collect(), 7, true, RepeatMode::Off).expect("valid start");
        assert_eq!(q.current(), 8);
        let mut seen = vec![q.current()];
        while let Some(id) = q.skip_next() {
            seen.push(id);
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=50).collect::<Vec<_>>());
    }

    #[test]
    fn unshuffle_returns_to_source_order_at_current_track() {
        let mut q = PlayQueue::new(vec![1, 2, 3, 4], 0, true, RepeatMode::Off).expect("valid start");
        q.skip_next();
        let current = q.current();
        q.set_shuffle(false);
        assert_eq!(q.current(), current);
        assert_eq!(q.peek_next(), [1, 2, 3, 4].iter().copied().find(|&id| id == current + 1));
    }
}
