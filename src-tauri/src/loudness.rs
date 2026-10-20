use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antistreaming_library::{Library, Loudness};
use tauri::{AppHandle, Manager};

use crate::library::LibraryState;

const BATCH: usize = 50;
/// How often a finished measurer looks for songs added since.
const IDLE_CHECK: Duration = Duration::from_secs(300);

/// Sound Check's measuring half: works through the library one song at a time while Sound Check
/// is on, so every song has a loudness to level against. Songs whose tags already state one are
/// never decoded.
#[derive(Default)]
pub struct LoudnessState {
    running: Mutex<Option<Arc<AtomicBool>>>,
}

pub fn set_measuring(app: &AppHandle, on: bool) {
    let state = app.state::<LoudnessState>();
    let mut running = state.running.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cancel) = running.take() {
        cancel.store(true, Ordering::Relaxed);
    }
    if !on {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *running = Some(Arc::clone(&cancel));
    let app = app.clone();
    std::thread::spawn(move || measure(&app, &cancel));
}

fn measure(app: &AppHandle, cancel: &AtomicBool) {
    let library = app.state::<LibraryState>();
    let Ok(reader) = Library::open(library.db_path()) else { return };
    // Silent, unreadable or too-short songs can't be measured; skipping them for this session
    // keeps them from being retried endlessly.
    let mut unmeasurable: HashSet<i64> = HashSet::new();
    while !cancel.load(Ordering::Relaxed) {
        let batch: Vec<_> = reader
            .tracks_needing_loudness(BATCH + unmeasurable.len())
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, _)| !unmeasurable.contains(id))
            .collect();
        if batch.is_empty() {
            for _ in 0..IDLE_CHECK.as_secs() {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            continue;
        }
        for (id, path) in batch {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let measured = antistreaming_playback::analyze_loudness(&path, &mut |_, _| {}, cancel)
                .ok()
                .filter(|l| l.integrated_lufs.is_finite() && l.true_peak_dbtp.is_finite());
            let saved = measured.is_some_and(|l| {
                let loudness =
                    Loudness { lufs: l.integrated_lufs, true_peak_dbtp: l.true_peak_dbtp, state: serde_json::to_vec(&l.blocks).ok() };
                library.with_writer(|library| library.set_loudness(id, &loudness)).is_ok()
            });
            if !saved {
                unmeasurable.insert(id);
            }
        }
    }
}
