use std::path::PathBuf;

use crate::gain::Gain;

/// What the caller knows about a track beyond its file, given when it is played or queued.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackOptions {
    /// Sound Check's adjustment for the track.
    pub gain: Option<Gain>,
    /// Names the album the track is from, in whatever form the caller likes. Consecutive
    /// tracks with the same key are never crossfaded, so that an album plays as it was made.
    pub album_key: Option<String>,
}

impl From<Option<Gain>> for TrackOptions {
    fn from(gain: Option<Gain>) -> Self {
        TrackOptions {
            gain,
            album_key: None,
        }
    }
}

impl From<Gain> for TrackOptions {
    fn from(gain: Gain) -> Self {
        Some(gain).into()
    }
}

/// What the engine needs to remember about a track once its decoder has moved on.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub id: i64,
    pub path: PathBuf,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub options: TrackOptions,
}

pub struct Transition {
    pub ended: i64,
    pub started: TrackInfo,
}

/// Maps the output's consumed-frame counter to track positions and track changes.
///
/// Decoding runs ahead of what the listener hears, so a gapless handoff is recorded as the
/// frame index where the next track's first sample sits in the stream and only takes effect
/// once playback reaches that index.
pub struct Timeline {
    output_rate: u32,
    current: TrackInfo,
    /// Stream frame at which the current track was at `anchor_ms`.
    anchor_frame: u64,
    anchor_ms: u64,
    /// Decoding runs at most one track ahead.
    upcoming: Option<Upcoming>,
}

/// The track decoded ahead of the one playing.
struct Upcoming {
    /// Stream frame of its first sample.
    start_frame: u64,
    /// Stream frame at which it becomes the current track: its start when it follows
    /// gaplessly, the middle of the overlap when it is crossfaded in, where the listener
    /// passes from one track to the other.
    switch_frame: u64,
    track: TrackInfo,
}

impl Timeline {
    pub fn new(output_rate: u32, current: TrackInfo, frame: u64, position_ms: u64) -> Self {
        Timeline {
            output_rate,
            current,
            anchor_frame: frame,
            anchor_ms: position_ms,
            upcoming: None,
        }
    }

    pub fn current(&self) -> &TrackInfo {
        &self.current
    }

    pub fn upcoming(&self) -> Option<&TrackInfo> {
        self.upcoming.as_ref().map(|upcoming| &upcoming.track)
    }

    /// Position in the source track, in the track's own milliseconds.
    pub fn position_ms(&self, consumed: u64) -> u64 {
        let played = u128::from(consumed.saturating_sub(self.anchor_frame));
        let elapsed = (played * 1000 / u128::from(self.output_rate)) as u64;
        let position = self.anchor_ms + elapsed;
        match self.current.duration_ms {
            0 => position,
            duration => position.min(duration),
        }
    }

    /// Records that `next` begins at stream frame `at_frame`.
    pub fn schedule(&mut self, at_frame: u64, next: TrackInfo) {
        self.schedule_crossfade(at_frame, at_frame, next);
    }

    /// Records that `next` begins at stream frame `start_frame`, fading in over the end of the
    /// current track, and takes over as the current track at `switch_frame`.
    pub fn schedule_crossfade(&mut self, start_frame: u64, switch_frame: u64, next: TrackInfo) {
        self.upcoming = Some(Upcoming {
            start_frame,
            switch_frame,
            track: next,
        });
    }

    pub fn take_upcoming(&mut self) -> Option<TrackInfo> {
        self.upcoming.take().map(|upcoming| upcoming.track)
    }

    /// Re-bases the current track after a seek or restart: it is at `position_ms` at `frame`.
    pub fn reanchor(&mut self, frame: u64, position_ms: u64) {
        self.anchor_frame = frame;
        self.anchor_ms = position_ms;
    }

    /// Switches to the upcoming track once playback has reached the frame where it takes over.
    pub fn advance(&mut self, consumed: u64) -> Option<Transition> {
        if consumed < self.upcoming.as_ref()?.switch_frame {
            return None;
        }
        let Upcoming {
            start_frame,
            track: started,
            ..
        } = self.upcoming.take()?;
        let ended = std::mem::replace(&mut self.current, started.clone());
        self.anchor_frame = start_frame;
        self.anchor_ms = 0;
        Some(Transition {
            ended: ended.id,
            started,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, duration_ms: u64) -> TrackInfo {
        TrackInfo {
            id,
            path: PathBuf::from(format!("{id}.flac")),
            duration_ms,
            sample_rate: 44_100,
            options: TrackOptions::default(),
        }
    }

    #[test]
    fn position_follows_consumed_frames_in_source_milliseconds() {
        let timeline = Timeline::new(48_000, track(1, 10_000), 0, 0);
        assert_eq!(timeline.position_ms(0), 0);
        assert_eq!(timeline.position_ms(48_000), 1_000);
        assert_eq!(timeline.position_ms(24_000 + 480), 510);
    }

    #[test]
    fn position_starts_from_the_anchor_after_a_seek() {
        let mut timeline = Timeline::new(44_100, track(1, 60_000), 0, 0);
        // The stream counter keeps running across a seek; only the anchor moves.
        timeline.reanchor(88_200, 30_000);
        assert_eq!(timeline.position_ms(88_200), 30_000);
        assert_eq!(timeline.position_ms(88_200 + 22_050), 30_500);
    }

    #[test]
    fn position_never_exceeds_a_known_duration() {
        let timeline = Timeline::new(44_100, track(1, 1_000), 0, 0);
        assert_eq!(timeline.position_ms(44_100 * 3), 1_000);
    }

    #[test]
    fn unknown_duration_leaves_position_unclamped() {
        let timeline = Timeline::new(44_100, track(1, 0), 0, 0);
        assert_eq!(timeline.position_ms(44_100 * 3), 3_000);
    }

    #[test]
    fn transition_fires_exactly_when_the_boundary_frame_is_consumed() {
        let mut timeline = Timeline::new(44_100, track(1, 1_000), 0, 0);
        timeline.schedule(44_100, track(2, 2_000));

        assert!(timeline.advance(44_099).is_none());
        assert_eq!(timeline.current().id, 1);

        let transition = timeline.advance(44_100).expect("boundary reached");
        assert_eq!((transition.ended, transition.started.id), (1, 2));
        assert_eq!(timeline.current().id, 2);
        assert!(timeline.advance(44_100).is_none());
    }

    #[test]
    fn position_after_a_transition_counts_from_the_boundary_not_from_when_it_was_noticed() {
        let mut timeline = Timeline::new(44_100, track(1, 1_000), 0, 0);
        timeline.schedule(44_100, track(2, 2_000));
        // The engine polls late: 10 ms of the next track have already played.
        timeline.advance(44_100 + 441);
        assert_eq!(timeline.position_ms(44_100 + 441), 10);
    }

    #[test]
    fn a_seek_back_does_not_inherit_the_old_track_boundary() {
        let mut timeline = Timeline::new(44_100, track(1, 1_000), 0, 0);
        timeline.schedule(44_100, track(2, 2_000));
        assert_eq!(timeline.take_upcoming().map(|track| track.id), Some(2));
        assert!(timeline.upcoming().is_none());
        assert!(timeline.advance(1_000_000).is_none());
    }

    #[test]
    fn a_crossfade_switches_tracks_midway_but_counts_the_new_one_from_its_first_frame() {
        let mut timeline = Timeline::new(44_100, track(1, 10_000), 0, 0);
        // The new track starts at frame 44100 and fades in over half a second.
        timeline.schedule_crossfade(44_100, 44_100 + 11_025, track(2, 10_000));

        assert!(timeline.advance(44_100 + 11_024).is_none());
        let transition = timeline.advance(44_100 + 11_025).expect("midway through");
        assert_eq!((transition.ended, transition.started.id), (1, 2));
        assert_eq!(timeline.position_ms(44_100 + 11_025), 250);
    }
}
