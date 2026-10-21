use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use unsub_library::{Library, MediaKind, SortKey, Track, TrackQuery};

use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;
use crate::playback::{PlaySource, PlaybackState, Playing, Tracks};
use crate::queue::{PlayQueue, RepeatMode};

pub const PARTY_EVENT: &str = "party://changed";

/// What Party Shuffle draws from: the whole music library or one playlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PartySource {
    #[default]
    Library,
    Playlist {
        id: i64,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartySettings {
    pub source: PartySource,
    pub recent: usize,
    pub upcoming: usize,
    pub favor_rated: bool,
}

impl Default for PartySettings {
    // iTunes DJ's defaults: 5 recently played, 15 upcoming.
    fn default() -> Self {
        Self { source: PartySource::Library, recent: 5, upcoming: 15, favor_rated: false }
    }
}

/// iTunes' Party Shuffle / iTunes DJ: a rolling queue that never runs dry.
#[derive(Debug, Default)]
pub struct PartyShuffle {
    pub settings: PartySettings,
    pub history: VecDeque<i64>,
    pub current: Option<i64>,
    pub upcoming: Vec<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyView {
    pub settings: PartySettings,
    pub history: Vec<Track>,
    pub current: Option<Track>,
    pub upcoming: Vec<Track>,
}

/// An upcoming row as it was when it was chosen: where it stood and which song it held. The queue moves on
/// under an open menu, so an action names the song as well and is skipped where it no longer stands.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpcomingPick {
    pub position: usize,
    pub track_id: i64,
}

/// Higher-rated songs come up more often when asked: an unrated song has weight 1, five stars 6.
fn weight(rating: u8, favor_rated: bool) -> u32 {
    if favor_rated { 1 + u32::from(rating) / 20 } else { 1 }
}

impl PartyShuffle {
    fn candidates(&self, library: &Library) -> unsub_library::Result<Vec<Track>> {
        let (kind, playlist) = match self.settings.source {
            PartySource::Library => (Some(MediaKind::Music), None),
            PartySource::Playlist { id } => (None, Some(id)),
        };
        let query = TrackQuery { kind, playlist, sort: SortKey::Artist, ..TrackQuery::default() };
        let busy: Vec<i64> = self.history.iter().chain(&self.upcoming).chain(&self.current).copied().collect();
        Ok(library.tracks(&query)?.into_iter().filter(|t| t.enabled && !busy.contains(&t.id)).collect())
    }

    /// Tops up the upcoming list with weighted random picks, never repeating what's queued or just played.
    pub fn refill(&mut self, library: &Library) -> unsub_library::Result<()> {
        let missing = self.settings.upcoming.saturating_sub(self.upcoming.len());
        if missing == 0 {
            return Ok(());
        }
        let mut pool = self.candidates(library)?;
        for _ in 0..missing {
            let total: u32 = pool.iter().map(|t| weight(t.rating, self.settings.favor_rated)).sum();
            if total == 0 {
                break;
            }
            let mut roll = fastrand::u32(0..total);
            let index = pool
                .iter()
                .position(|t| {
                    let w = weight(t.rating, self.settings.favor_rated);
                    if roll < w {
                        return true;
                    }
                    roll -= w;
                    false
                })
                .unwrap_or(0);
            self.upcoming.push(pool.swap_remove(index).id);
        }
        Ok(())
    }

    /// The song chosen by `pick`, if it is still where it was chosen.
    fn picked(&self, pick: UpcomingPick) -> Option<i64> {
        self.upcoming.get(pick.position).copied().filter(|&id| id == pick.track_id)
    }

    /// Takes out the chosen songs that are still where they were chosen; the rest are left alone.
    fn remove_picked(&mut self, picks: &[UpcomingPick]) {
        let doomed: Vec<usize> = picks.iter().filter(|&&pick| self.picked(pick).is_some()).map(|pick| pick.position).collect();
        let mut index = 0;
        self.upcoming.retain(|_| {
            let keep = !doomed.contains(&index);
            index += 1;
            keep
        });
    }

    /// The engine started `track_id`: move the old current song into history and step forward.
    pub fn advance_to(&mut self, track_id: i64) {
        if self.current == Some(track_id) {
            return;
        }
        if let Some(previous) = self.current.take() {
            self.history.push_back(previous);
        }
        while self.history.len() > self.settings.recent {
            self.history.pop_front();
        }
        if let Some(position) = self.upcoming.iter().position(|&id| id == track_id) {
            self.upcoming.drain(..=position);
        }
        self.current = Some(track_id);
    }

    /// Clears the upcoming list and draws a fresh one, as iTunes' Refresh button did.
    pub fn reshuffle(&mut self, library: &Library) -> unsub_library::Result<()> {
        self.upcoming.clear();
        self.refill(library)
    }

    pub fn view(&self, library: &Library) -> unsub_library::Result<PartyView> {
        let load = |ids: &mut dyn Iterator<Item = &i64>| -> unsub_library::Result<Vec<Track>> {
            ids.filter_map(|&id| library.track(id).transpose()).collect()
        };
        Ok(PartyView {
            settings: self.settings,
            history: load(&mut self.history.iter())?,
            current: self.current.map(|id| library.track(id)).transpose()?.flatten(),
            upcoming: load(&mut self.upcoming.iter())?,
        })
    }
}

/// The queue the engine sees in party mode: what's playing, then what's coming.
fn party_queue(party: &PartyShuffle) -> Option<PlayQueue> {
    let current = party.current?;
    PlayQueue::new(std::iter::once(current).chain(party.upcoming.iter().copied()).collect(), 0, false, RepeatMode::Off)
}

/// Re-points a playing party at the (edited) upcoming list so the engine queues the right next song.
fn requeue(playback: &PlaybackState, tracks: &Tracks, slot: &mut Option<Playing>, party: &PartyShuffle) -> CommandResult<()> {
    match slot.as_mut() {
        Some(playing) if playing.party => {
            if let Some(queue) = party_queue(party) {
                playing.queue = queue;
                playback.queue_following(tracks, playing)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Engine thread (via the event worker): step the party forward and keep it topped up.
pub(crate) fn on_track_started(
    app: &AppHandle,
    playback: &PlaybackState,
    tracks: &Tracks,
    playing: &mut Playing,
    track_id: i64,
) -> CommandResult<()> {
    let mut party = playback.party();
    party.advance_to(track_id);
    party.refill(&tracks.library.queries())?;
    if let Some(queue) = party_queue(&party) {
        playing.queue = queue;
        playback.queue_following(tracks, playing)?;
    }
    let _ = app.emit(PARTY_EVENT, ());
    Ok(())
}

fn view(playback: &PlaybackState, library: &LibraryState) -> CommandResult<PartyView> {
    Ok(playback.party().view(&library.queries())?)
}

#[tauri::command]
pub fn party_view(playback: State<'_, PlaybackState>, library: State<'_, LibraryState>) -> CommandResult<PartyView> {
    {
        let mut party = playback.party();
        party.refill(&library.queries())?;
    }
    view(&playback, &library)
}

/// Starts the party: from a chosen upcoming song (double-click), or picks up where it left off. A chosen song
/// that is no longer where it was chosen starts nothing.
#[tauri::command]
pub fn party_play(
    app: AppHandle,
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    pick: Option<UpcomingPick>,
) -> CommandResult<PartyView> {
    let tracks = Tracks { library: &library, devices: &devices };
    let mut slot = playback.playing();
    {
        let mut party = playback.party();
        party.refill(&library.queries())?;
        let nothing_to_play = || CommandError::from("Party Shuffle has nothing to play".to_owned());
        let start = match pick {
            Some(pick) => party.picked(pick),
            None => Some(party.current.or_else(|| party.upcoming.first().copied()).ok_or_else(nothing_to_play)?),
        };
        if let Some(start) = start {
            party.advance_to(start);
            party.refill(&library.queries())?;
            let queue = party_queue(&party).ok_or_else(nothing_to_play)?;
            let playing = Playing { source: PlaySource::Library, queue, party: true };
            playback.start(&tracks, &playing)?;
            *slot = Some(playing);
        }
    }
    drop(slot);
    let _ = app.emit(PARTY_EVENT, ());
    view(&playback, &library)
}

/// Runs an edit on the party, re-queues the engine if it's playing, and returns the new view.
fn edit(
    app: &AppHandle,
    playback: &PlaybackState,
    library: &LibraryState,
    devices: &DevicesState,
    change: impl FnOnce(&mut PartyShuffle, &Library) -> unsub_library::Result<()>,
) -> CommandResult<PartyView> {
    let tracks = Tracks { library, devices };
    let mut slot = playback.playing();
    {
        let mut party = playback.party();
        change(&mut party, &library.queries())?;
        party.refill(&library.queries())?;
        requeue(playback, &tracks, &mut slot, &party)?;
    }
    drop(slot);
    let _ = app.emit(PARTY_EVENT, ());
    view(playback, library)
}

#[tauri::command]
pub fn party_set_settings(
    app: AppHandle,
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    settings: PartySettings,
) -> CommandResult<PartyView> {
    edit(&app, &playback, &library, &devices, |party, library| {
        let source_changed = party.settings.source != settings.source;
        party.settings = settings;
        party.upcoming.truncate(settings.upcoming);
        while party.history.len() > settings.recent {
            party.history.pop_front();
        }
        // A new source means a new crowd: redraw what's coming, as iTunes did.
        if source_changed { party.reshuffle(library) } else { Ok(()) }
    })
}

/// Adds songs to the upcoming list at `at` (0 is "Play Next"), or at the end.
#[tauri::command]
pub fn party_add(
    app: AppHandle,
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    track_ids: Vec<i64>,
    at: Option<usize>,
) -> CommandResult<PartyView> {
    edit(&app, &playback, &library, &devices, |party, _| {
        let at = at.unwrap_or(party.upcoming.len()).min(party.upcoming.len());
        party.upcoming.splice(at..at, track_ids);
        Ok(())
    })
}

#[tauri::command]
pub fn party_remove(
    app: AppHandle,
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    picks: Vec<UpcomingPick>,
) -> CommandResult<PartyView> {
    edit(&app, &playback, &library, &devices, |party, _| {
        party.remove_picked(&picks);
        Ok(())
    })
}

#[tauri::command]
pub fn party_move(
    app: AppHandle,
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
    positions: Vec<usize>,
    to: usize,
) -> CommandResult<PartyView> {
    edit(&app, &playback, &library, &devices, |party, _| {
        let moving: Vec<i64> = positions.iter().filter_map(|&p| party.upcoming.get(p).copied()).collect();
        let before = positions.iter().filter(|&&p| p < to).count();
        let mut index = 0;
        party.upcoming.retain(|_| {
            let keep = !positions.contains(&index);
            index += 1;
            keep
        });
        let at = to.saturating_sub(before).min(party.upcoming.len());
        party.upcoming.splice(at..at, moving);
        Ok(())
    })
}

#[tauri::command]
pub fn party_reshuffle(
    app: AppHandle,
    playback: State<'_, PlaybackState>,
    library: State<'_, LibraryState>,
    devices: State<'_, DevicesState>,
) -> CommandResult<PartyView> {
    edit(&app, &playback, &library, &devices, |party, library| party.reshuffle(library))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advancing_moves_current_into_bounded_history() {
        let mut party = PartyShuffle { upcoming: vec![1, 2, 3, 4], ..PartyShuffle::default() };
        party.settings.recent = 2;
        for id in [1, 2, 3, 4] {
            party.advance_to(id);
        }
        assert_eq!(party.current, Some(4));
        assert_eq!(party.history, VecDeque::from([2, 3]));
        assert!(party.upcoming.is_empty());
    }

    #[test]
    fn advancing_to_a_later_song_drops_the_skipped_ones() {
        let mut party = PartyShuffle { upcoming: vec![1, 2, 3], ..PartyShuffle::default() };
        party.advance_to(3);
        assert!(party.upcoming.is_empty());
        assert_eq!(party.current, Some(3));
    }

    #[test]
    fn advancing_to_the_current_song_is_a_no_op() {
        let mut party = PartyShuffle { current: Some(7), upcoming: vec![8], ..PartyShuffle::default() };
        party.advance_to(7);
        assert_eq!(party.upcoming, vec![8]);
        assert!(party.history.is_empty());
    }

    fn pick(position: usize, track_id: i64) -> UpcomingPick {
        UpcomingPick { position, track_id }
    }

    #[test]
    fn a_pick_names_a_song_only_where_it_still_stands() {
        let party = PartyShuffle { upcoming: vec![5, 6, 7], ..PartyShuffle::default() };
        assert_eq!(party.picked(pick(1, 6)), Some(6));
        // The queue moved up one while the menu was open: 6 now stands at 0, and 7 where 6 was.
        assert_eq!(party.picked(pick(2, 6)), None);
        assert_eq!(party.picked(pick(3, 7)), None);
    }

    #[test]
    fn removing_picks_skips_those_that_moved_and_keeps_the_rest_in_order() {
        let mut party = PartyShuffle { upcoming: vec![1, 2, 3, 4, 5], ..PartyShuffle::default() };
        party.remove_picked(&[pick(1, 2), pick(2, 9), pick(4, 3), pick(7, 5), pick(3, 4)]);
        assert_eq!(party.upcoming, vec![1, 3, 5]);
    }

    #[test]
    fn rating_weights_only_apply_when_favoured() {
        assert_eq!(weight(100, false), 1);
        assert_eq!(weight(100, true), 6);
        assert_eq!(weight(0, true), 1);
    }
}
