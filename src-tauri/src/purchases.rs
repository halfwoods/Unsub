use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
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

/// Commands touch the disk, the ledger and the network. A synchronous command runs on the main thread, so any
/// wait inside one freezes the window; these run on the threads made for waiting instead.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> CommandResult<T> + Send + 'static) -> CommandResult<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|error| CommandError::from(error.to_string()))?
}

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    watching: bool,
}

/// The Purchases inbox. Watching ~/Downloads is opt-in: macOS guards that folder, and the
/// permission prompt should follow the user's click, not app launch.
pub struct PurchasesState {
    /// Held for as long as a scan or an extraction takes, so nothing the window asks for may wait on it.
    inbox: Mutex<Inbox>,
    /// A second connection to the same ledger for the quick reads and writes the window makes, so it never queues
    /// behind a scan, which sits on a macOS permission prompt the first time it reads ~/Downloads.
    ledger: Mutex<Inbox>,
    downloads: PathBuf,
    settings_path: PathBuf,
    watcher: Mutex<Option<WatchHandle>>,
}

impl PurchasesState {
    pub fn open(data_dir: &std::path::Path, library_dir: PathBuf, downloads: PathBuf) -> unsub_purchases::Result<Self> {
        let ledger_path = data_dir.join("Purchases.sqlite");
        Ok(Self {
            inbox: Mutex::new(Inbox::open(&ledger_path, &library_dir)?),
            ledger: Mutex::new(Inbox::open(&ledger_path, &library_dir)?),
            downloads,
            settings_path: data_dir.join("Purchases.json"),
            watcher: Mutex::new(None),
        })
    }

    fn watching(&self) -> bool {
        std::fs::read(&self.settings_path).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()).is_some_and(|s| s.watching)
    }

    /// Starts or stops the Downloads watcher; each change triggers a scan for new purchases. It works on its own
    /// thread because the first look at ~/Downloads can wait on the user answering a macOS prompt, and the scan
    /// for what arrived while nothing was watching takes as long as the folder is big.
    pub fn apply(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            let state = app.state::<PurchasesState>();
            // Re-reading the setting under the lock leaves the latest click in force when applies overlap.
            let mut watcher = lock(&state.watcher);
            *watcher = None;
            if !state.watching() {
                return;
            }
            let handle = app.clone();
            *watcher = unsub_purchases::watch(vec![state.downloads.clone()], move || scan(&handle))
                .map_err(|error| eprintln!("[purchases] can't watch Downloads: {error}"))
                .ok();
            drop(watcher);
            scan(&app);
        });
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
pub async fn purchases_status(app: AppHandle) -> CommandResult<PurchasesStatus> {
    blocking(move || {
        let state = app.state::<PurchasesState>();
        let ledger = lock(&state.ledger);
        Ok(PurchasesStatus {
            watching: state.watching(),
            downloads: state.downloads.to_string_lossy().into_owned(),
            candidates: ledger.candidates().map_err(to_error)?,
            purchases: ledger.purchases().map_err(to_error)?,
        })
    })
    .await
}

/// Answers as soon as the setting is saved; the watcher and its first scan follow in the background and report
/// through the changed event, which the window is also sent now so it can show that watching has begun.
#[tauri::command]
pub async fn purchases_set_watching(app: AppHandle, watching: bool) -> CommandResult<()> {
    blocking(move || {
        let state = app.state::<PurchasesState>();
        let json = serde_json::to_vec(&Settings { watching }).map_err(|e| CommandError::from(e.to_string()))?;
        std::fs::write(&state.settings_path, json).map_err(|e| CommandError::from(e.to_string()))?;
        PurchasesState::apply(&app);
        let _ = app.emit(CHANGED_EVENT, ());
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn purchases_scan(app: AppHandle) {
    std::thread::spawn(move || scan(&app));
}

/// Extracts the purchase into the library folder, then imports it like any added folder.
#[tauri::command]
pub async fn purchases_accept(app: AppHandle, candidate_id: i64) -> CommandResult<Purchase> {
    blocking(move || {
        let state = app.state::<PurchasesState>();
        let purchase = lock(&state.inbox).accept(candidate_id, &mut |_, _| {}).map_err(to_error)?;
        library::add_paths(&app, purchase.files.clone());
        let _ = app.emit(CHANGED_EVENT, ());
        Ok(purchase)
    })
    .await
}

#[tauri::command]
pub async fn purchases_ignore(app: AppHandle, candidate_id: i64) -> CommandResult<()> {
    blocking(move || {
        lock(&app.state::<PurchasesState>().ledger).ignore(candidate_id).map_err(to_error)?;
        let _ = app.emit(CHANGED_EVENT, ());
        Ok(())
    })
    .await
}

/// Real "where to buy" links from MusicBrainz for an album (rate-limited and cached by the ledger).
#[tauri::command]
pub async fn purchases_links(app: AppHandle, artist: String, album: String) -> CommandResult<Vec<StoreLink>> {
    blocking(move || unsub_purchases::purchase_links(&artist, &album, &app.state::<PurchasesState>().ledger).map_err(to_error)).await
}
