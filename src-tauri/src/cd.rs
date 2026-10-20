use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use antistreaming_cdrip::{AccurateRip, Disc, Format, Release, RipProgress, RipSettings, UreqClient};
use antistreaming_library::{MediaKind, Track};
use antistreaming_playback::PlayerEvent;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::devices::DevicesState;
use crate::error::{CommandError, CommandResult};
use crate::library::{self, LibraryState};
use crate::playback::{PlaySource, PlaybackState, PLAYER_EVENT};

pub const CHANGED_EVENT: &str = "cd://changed";
/// Import progress while a CD rips; `null` once it stops.
pub const RIP_EVENT: &str = "cd://rip";

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// A CD sector holds 2,352 bytes of 16-bit stereo PCM: the size of a track as the AIFF it mounts as.
const BYTES_PER_SECTOR: u64 = 2_352;
/// The flac tool's default level: nearly the smallest files at a fraction of level 8's time.
const FLAC_COMPRESSION: u8 = 5;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn to_error(error: antistreaming_cdrip::Error) -> CommandError {
    CommandError::from(error.to_string())
}

struct Inserted {
    disc: Disc,
    /// MusicBrainz's matches; `None` while the lookup is still out.
    lookup: Option<Result<Vec<Release>, String>>,
    /// Which match names the tracks. iTunes asked when several releases shared one disc.
    chosen: usize,
    /// iTunes' checkboxes: which tracks Import CD takes. All start checked.
    checked: HashSet<u8>,
    imported: Vec<ImportedTrack>,
}

impl Inserted {
    fn release(&self) -> Option<&Release> {
        self.lookup.as_ref()?.as_ref().ok()?.get(self.chosen)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedTrack {
    number: u8,
    accuraterip: AccurateRipResult,
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
enum AccurateRipResult {
    Matched { confidence: u32 },
    NotInDatabase,
    Mismatch,
    Skipped,
}

impl From<AccurateRip> for AccurateRipResult {
    fn from(result: AccurateRip) -> Self {
        match result {
            AccurateRip::Matched { confidence } => Self::Matched { confidence },
            AccurateRip::NotInDatabase => Self::NotInDatabase,
            AccurateRip::Mismatch => Self::Mismatch,
            AccurateRip::Skipped => Self::Skipped,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RipStatus {
    disc_id: String,
    /// The track being read now, then the ones still waiting.
    current: u8,
    title: String,
    queued: Vec<u8>,
    fraction: f32,
    /// Which of the tracks being imported this is, from 0, and how many there are.
    index: usize,
    count: usize,
}

struct Rip {
    status: RipStatus,
    titles: Vec<(u8, String)>,
    cancel: Arc<AtomicBool>,
    /// Eject was chosen mid-import: stop, then eject once the drive is free.
    eject_after: bool,
}

/// Audio CDs in the drive, kept with the iPods because iTunes lists both under DEVICES.
#[derive(Default)]
pub struct Cds {
    inserted: Mutex<Vec<Inserted>>,
    rip: Mutex<Option<Rip>>,
}

impl Cds {
    fn inserted(&self) -> MutexGuard<'_, Vec<Inserted>> {
        lock(&self.inserted)
    }

    fn rip(&self) -> MutexGuard<'_, Option<Rip>> {
        lock(&self.rip)
    }

    /// CD tracks are identified by their number on the disc.
    pub fn track_path(&self, disc_id: &str, track_id: i64) -> CommandResult<PathBuf> {
        self.inserted()
            .iter()
            .find(|cd| cd.disc.id == disc_id)
            .and_then(|cd| cd.disc.tracks.iter().find(|t| i64::from(t.number) == track_id))
            .map(|track| track.path.clone())
            .ok_or_else(|| CommandError::from(format!("track {track_id} is not on the CD")))
    }

    fn with_cd<T>(&self, disc_id: &str, f: impl FnOnce(&mut Inserted) -> T) -> CommandResult<T> {
        let mut inserted = self.inserted();
        let cd = inserted
            .iter_mut()
            .find(|cd| cd.disc.id == disc_id)
            .ok_or_else(|| CommandError::from("that CD is no longer in the drive".to_owned()))?;
        Ok(f(cd))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdRelease {
    title: String,
    artist: String,
    year: Option<i32>,
    disc_number: u32,
    disc_count: u32,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum LookupState {
    Pending,
    Found,
    NotFound,
    Failed { message: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CdSummary {
    id: String,
    /// The album when MusicBrainz knows the disc, otherwise what the drive calls it.
    name: String,
    track_count: usize,
    lookup: LookupState,
    releases: Vec<CdRelease>,
    chosen: usize,
    cover_art_url: Option<String>,
    imported: Vec<ImportedTrack>,
    importing: Option<RipStatus>,
}

fn summary(cd: &Inserted, rip: Option<&Rip>) -> CdSummary {
    let releases = match &cd.lookup {
        Some(Ok(releases)) => releases.as_slice(),
        _ => &[],
    };
    let lookup = match &cd.lookup {
        None => LookupState::Pending,
        Some(Ok(releases)) if releases.is_empty() => LookupState::NotFound,
        Some(Ok(_)) => LookupState::Found,
        Some(Err(message)) => LookupState::Failed { message: message.clone() },
    };
    CdSummary {
        id: cd.disc.id.clone(),
        name: cd.release().map_or_else(|| cd.disc.name.clone(), |r| r.title.clone()),
        track_count: cd.disc.tracks.len(),
        lookup,
        releases: releases
            .iter()
            .map(|r| CdRelease {
                title: r.title.clone(),
                artist: r.artist.clone(),
                year: r.year,
                disc_number: r.disc_number,
                disc_count: r.disc_count,
            })
            .collect(),
        chosen: cd.chosen,
        cover_art_url: cd.release().and_then(|r| r.cover_art_url.clone()),
        imported: cd.imported.clone(),
        importing: rip.filter(|rip| rip.status.disc_id == cd.disc.id).map(|rip| rip.status.clone()),
    }
}

fn tracks(cd: &Inserted) -> Vec<Track> {
    let tags = antistreaming_cdrip::describe_tracks(&cd.disc, cd.release());
    cd.disc
        .tracks
        .iter()
        .zip(tags)
        .map(|(track, tags)| Track {
            id: i64::from(track.number),
            path: track.path.to_string_lossy().into_owned(),
            title: tags.title,
            artist: tags.artist,
            album_artist: tags.album_artist,
            album: tags.album,
            genre: String::new(),
            composer: String::new(),
            year: tags.year,
            track_number: Some(tags.track),
            track_count: Some(tags.track_total),
            disc_number: Some(tags.disc),
            disc_count: Some(tags.disc_total),
            duration_ms: (track.duration_seconds() * 1000.0).round() as u64,
            bitrate_kbps: Some(1_411),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            codec: "AIFF".to_owned(),
            file_size: u64::from(track.sectors) * BYTES_PER_SECTOR,
            date_added: 0,
            date_modified: 0,
            play_count: 0,
            last_played: None,
            skip_count: 0,
            rating: 0,
            kind: MediaKind::Music,
            enabled: cd.checked.contains(&track.number),
            in_cloud: false,
            missing: false,
            loudness: None,
            compilation: false,
            sort_title: None,
            sort_artist: None,
            sort_album: None,
            sort_album_artist: None,
        })
        .collect()
}

fn title_of(titles: &[(u8, String)], number: u8) -> String {
    titles.iter().find(|(n, _)| *n == number).map(|(_, title)| title.clone()).unwrap_or_default()
}

fn changed(app: &AppHandle) {
    let _ = app.emit(CHANGED_EVENT, ());
}

/// Watches the drive for the life of the app; a new disc is looked up on MusicBrainz once.
pub fn watch(app: AppHandle) {
    std::thread::spawn(move || loop {
        let discs = antistreaming_cdrip::discs();
        let devices = app.state::<DevicesState>();
        let cds = &devices.cds;
        let gone: Vec<String> = cds
            .inserted()
            .iter()
            .filter(|cd| !discs.iter().any(|d| d.id == cd.disc.id))
            .map(|cd| cd.disc.id.clone())
            .collect();
        for disc_id in &gone {
            // Bound first so the CD lock is released before playback's (lock order).
            app.state::<PlaybackState>().stop_if_playing(&PlaySource::Cd { disc_id: disc_id.clone() });
            if let Some(rip) = cds.rip().as_ref().filter(|rip| &rip.status.disc_id == disc_id) {
                rip.cancel.store(true, Ordering::Relaxed);
            }
        }
        let new: Vec<Disc> = {
            let mut inserted = cds.inserted();
            inserted.retain(|cd| !gone.contains(&cd.disc.id));
            discs.into_iter().filter(|d| !inserted.iter().any(|cd| cd.disc.id == d.id)).collect()
        };
        for disc in &new {
            cds.inserted().push(Inserted {
                checked: disc.tracks.iter().map(|t| t.number).collect(),
                disc: disc.clone(),
                lookup: None,
                chosen: 0,
                imported: Vec::new(),
            });
            look_up(app.clone(), disc.clone());
        }
        if !gone.is_empty() || !new.is_empty() {
            changed(&app);
        }
        std::thread::sleep(POLL_INTERVAL);
    });
}

fn look_up(app: AppHandle, disc: Disc) {
    std::thread::spawn(move || {
        let found = antistreaming_cdrip::lookup(&disc, &UreqClient::new()).map_err(|e| e.to_string());
        let devices = app.state::<DevicesState>();
        if let Some(cd) = devices.cds.inserted().iter_mut().find(|cd| cd.disc.id == disc.id) {
            cd.lookup = Some(found);
        }
        changed(&app);
    });
}

#[tauri::command]
pub fn cd_list(state: State<'_, DevicesState>) -> Vec<CdSummary> {
    let rip = state.cds.rip();
    state.cds.inserted().iter().map(|cd| summary(cd, rip.as_ref())).collect()
}

#[tauri::command]
pub fn cd_tracks(state: State<'_, DevicesState>, disc_id: String) -> CommandResult<Vec<Track>> {
    state.cds.with_cd(&disc_id, |cd| tracks(cd))
}

#[tauri::command]
pub fn cd_set_checked(app: AppHandle, state: State<'_, DevicesState>, disc_id: String, numbers: Vec<u8>, checked: bool) -> CommandResult<()> {
    state.cds.with_cd(&disc_id, |cd| {
        for number in numbers {
            if checked {
                cd.checked.insert(number);
            } else {
                cd.checked.remove(&number);
            }
        }
    })?;
    changed(&app);
    Ok(())
}

#[tauri::command]
pub fn cd_choose_release(app: AppHandle, state: State<'_, DevicesState>, disc_id: String, index: usize) -> CommandResult<()> {
    state.cds.with_cd(&disc_id, |cd| cd.chosen = index)?;
    changed(&app);
    Ok(())
}

/// iTunes' Import Settings, chosen in the UI and sent with each import.
#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "codec", rename_all = "camelCase")]
pub enum ImportFormat {
    Flac,
    Alac,
    Aac { kbps: u32 },
}

impl From<ImportFormat> for Format {
    fn from(format: ImportFormat) -> Self {
        match format {
            ImportFormat::Flac => Format::Flac { compression: FLAC_COMPRESSION },
            ImportFormat::Alac => Format::Alac,
            ImportFormat::Aac { kbps } => Format::Aac { kbps },
        }
    }
}

/// Rips the checked tracks into the library folder; each joins the library as soon as it's done,
/// as in iTunes, so a stopped import keeps what it finished.
#[tauri::command]
pub fn cd_import(app: AppHandle, state: State<'_, DevicesState>, disc_id: String, format: ImportFormat, verify: bool) -> CommandResult<()> {
    let (disc, release, mut numbers) = state.cds.with_cd(&disc_id, |cd| {
        (cd.disc.clone(), cd.release().cloned(), cd.checked.iter().copied().collect::<Vec<u8>>())
    })?;
    let titles: Vec<(u8, String)> = disc
        .tracks
        .iter()
        .zip(antistreaming_cdrip::describe_tracks(&disc, release.as_ref()))
        .map(|(track, tags)| (track.number, tags.title))
        .collect();
    numbers.sort_unstable();
    let Some(&first) = numbers.first() else {
        return Err(CommandError::from("check the songs you want to import first".to_owned()));
    };
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut rip = state.cds.rip();
        if rip.is_some() {
            return Err(CommandError::from("a CD is already importing".to_owned()));
        }
        *rip = Some(Rip {
            status: RipStatus {
                disc_id: disc_id.clone(),
                current: first,
                title: title_of(&titles, first),
                queued: numbers[1..].to_vec(),
                fraction: 0.0,
                index: 0,
                count: numbers.len(),
            },
            titles,
            cancel: Arc::clone(&cancel),
            eject_after: false,
        });
    }
    let settings = RipSettings {
        format: format.into(),
        dest_dir: app
            .state::<LibraryState>()
            .media_folder(&app.path().audio_dir().map_err(|e| CommandError::from(e.to_string()))?.join("AntiStreaming"))?,
        verify_accuraterip: verify,
        tracks: Some(numbers),
    };
    let _ = app.emit(RIP_EVENT, state.cds.rip().as_ref().map(|rip| rip.status.clone()));
    changed(&app);

    std::thread::spawn(move || {
        let devices = app.state::<DevicesState>();
        let cds = &devices.cds;
        let mut on_progress = |event: RipProgress| {
            let status = {
                let mut rip = cds.rip();
                let Some(rip) = rip.as_mut() else { return };
                match &event {
                    RipProgress::TrackStarted { number, index, count } => {
                        rip.status.current = *number;
                        rip.status.title = title_of(&rip.titles, *number);
                        rip.status.queued.retain(|n| n != number);
                        rip.status.fraction = 0.0;
                        (rip.status.index, rip.status.count) = (*index, *count);
                    }
                    RipProgress::TrackProgress { fraction, .. } => rip.status.fraction = *fraction,
                    RipProgress::TrackFinished(_) => rip.status.fraction = 1.0,
                }
                rip.status.clone()
            };
            if let RipProgress::TrackFinished(track) = event {
                let _ = cds.with_cd(&status.disc_id, |cd| {
                    cd.imported.retain(|t| t.number != track.number);
                    cd.imported.push(ImportedTrack { number: track.number, accuraterip: track.accuraterip.into() });
                });
                library::add_paths(&app, vec![track.path]);
                changed(&app);
            }
            let _ = app.emit(RIP_EVENT, Some(status));
        };
        let result = antistreaming_cdrip::rip(&disc, release.as_ref(), &settings, &UreqClient::new(), &mut on_progress, &cancel);
        let eject_after = cds.rip().take().is_some_and(|rip| rip.eject_after);
        let _ = app.emit(RIP_EVENT, None::<RipStatus>);
        match result {
            Ok(_) | Err(antistreaming_cdrip::Error::Cancelled) => {}
            Err(error) => {
                let _ = app.emit(PLAYER_EVENT, PlayerEvent::Notice { message: format!("Import stopped: {error}") });
            }
        }
        if eject_after
            && let Err(error) = antistreaming_cdrip::eject(&disc)
        {
            let _ = app.emit(PLAYER_EVENT, PlayerEvent::Notice { message: error.to_string() });
        }
        changed(&app);
    });
    Ok(())
}

#[tauri::command]
pub fn cd_stop_import(state: State<'_, DevicesState>) {
    if let Some(rip) = state.cds.rip().as_ref() {
        rip.cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn cd_eject(app: AppHandle, state: State<'_, DevicesState>, disc_id: String) -> CommandResult<()> {
    // Our open files would make the drive refuse to eject.
    app.state::<PlaybackState>().stop_if_playing(&PlaySource::Cd { disc_id: disc_id.clone() });
    {
        let mut rip = state.cds.rip();
        if let Some(rip) = rip.as_mut().filter(|rip| rip.status.disc_id == disc_id) {
            rip.cancel.store(true, Ordering::Relaxed);
            rip.eject_after = true;
            return Ok(());
        }
    }
    let disc = state.cds.with_cd(&disc_id, |cd| cd.disc.clone())?;
    antistreaming_cdrip::eject(&disc).map_err(to_error)
}
