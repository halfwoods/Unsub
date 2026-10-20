use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use unsub_purchases::{Candidate, Inbox, Purchase, StoreLink, WatchHandle};

use crate::error::{CommandError, CommandResult};
use crate::library;

pub const CHANGED_EVENT: &str = "purchases://changed";

fn to_error(error: unsub_purchases::Error) -> CommandError {
    CommandError::from(error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    watching: bool,
}

/// The Purchases inbox. Watching ~/Downloads is opt-in: macOS guards that folder, and the
/// permission prompt should follow the user's click, not app launch.
pub struct PurchasesState {
    inbox: Mutex<Inbox>,
    downloads: PathBuf,
    settings_path: PathBuf,
    watcher: Mutex<Option<WatchHandle>>,
}

impl PurchasesState {
    pub fn open(data_dir: &std::path::Path, library_dir: PathBuf, downloads: PathBuf) -> unsub_purchases::Result<Self> {
        Ok(Self {
            inbox: Mutex::new(Inbox::open(&data_dir.join("Purchases.sqlite"), &library_dir)?),
            downloads,
            settings_path: data_dir.join("Purchases.json"),
            watcher: Mutex::new(None),
        })
    }

    fn watching(&self) -> bool {
        std::fs::read(&self.settings_path).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()).is_some_and(|s| s.watching)
    }

    /// Starts or stops the Downloads watcher; each change triggers a scan for new purchases.
    pub fn apply(&self, app: &AppHandle) {
        let mut watcher = lock(&self.watcher);
        *watcher = None;
        if !self.watching() {
            return;
        }
        let handle = app.clone();
        *watcher = unsub_purchases::watch(vec![self.downloads.clone()], move || scan(&handle))
            .map_err(|error| eprintln!("[purchases] can't watch Downloads: {error}"))
            .ok();
        drop(watcher);
        scan(app);
    }
}

fn scan(app: &AppHandle) {
    let state = app.state::<PurchasesState>();
    let found = lock(&state.inbox).scan(std::slice::from_ref(&state.downloads));
    match found {
        Ok(_) => {
            let _ = app.emit(CHANGED_EVENT, ());
        }
        Err(error) => eprintln!("[purchases] scan failed: {error}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchasesStatus {
    watching: bool,
    downloads: String,
    candidates: Vec<Candidate>,
    purchases: Vec<Purchase>,
}

#[tauri::command]
pub fn purchases_status(state: State<'_, PurchasesState>) -> CommandResult<PurchasesStatus> {
    let inbox = lock(&state.inbox);
    Ok(PurchasesStatus {
        watching: state.watching(),
        downloads: state.downloads.to_string_lossy().into_owned(),
        candidates: inbox.candidates().map_err(to_error)?,
        purchases: inbox.purchases().map_err(to_error)?,
    })
}

#[tauri::command]
pub fn purchases_set_watching(app: AppHandle, state: State<'_, PurchasesState>, watching: bool) -> CommandResult<()> {
    let json = serde_json::to_vec(&Settings { watching }).map_err(|e| CommandError::from(e.to_string()))?;
    std::fs::write(&state.settings_path, json).map_err(|e| CommandError::from(e.to_string()))?;
    state.apply(&app);
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

#[tauri::command]
pub fn purchases_scan(app: AppHandle) {
    std::thread::spawn(move || scan(&app));
}

/// Extracts the purchase into the library folder, then imports it like any added folder.
#[tauri::command]
pub async fn purchases_accept(app: AppHandle, state: State<'_, PurchasesState>, candidate_id: i64) -> CommandResult<Purchase> {
    let purchase = lock(&state.inbox).accept(candidate_id, &mut |_, _| {}).map_err(to_error)?;
    library::add_paths(&app, purchase.files.clone());
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(purchase)
}

#[tauri::command]
pub fn purchases_ignore(app: AppHandle, state: State<'_, PurchasesState>, candidate_id: i64) -> CommandResult<()> {
    lock(&state.inbox).ignore(candidate_id).map_err(to_error)?;
    let _ = app.emit(CHANGED_EVENT, ());
    Ok(())
}

/// Real "where to buy" links from MusicBrainz for an album (rate-limited and cached by the inbox).
#[tauri::command]
pub async fn purchases_links(state: State<'_, PurchasesState>, artist: String, album: String) -> CommandResult<Vec<StoreLink>> {
    let inbox = lock(&state.inbox);
    unsub_purchases::purchase_links(&artist, &album, &inbox).map_err(to_error)
}
