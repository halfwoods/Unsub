//! Updates through tauri-plugin-updater: a check at launch and while the app stays open (at most
//! once a day), "Check for Updates…" in the app menu, and the download and install behind the UI's
//! update sheet.
//!
//! Nothing here contacts anyone unless tauri.conf.json carries a real public key and endpoint
//! (`configured`), so a build made without the maintainer's signing key never offers updates and
//! never fails because of that.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{CommandError, CommandResult};

/// The UI listens to this for the outcome of every check.
pub const RESULT_EVENT: &str = "updates://result";

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// How often the watcher asks whether a check is due; the daily interval is enforced by the gate,
/// this only bounds how late into a day a long-running app notices it has passed.
const WATCH_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Keeps a hung request from still running when the watcher's next pass asks again.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// Every minisign public key file starts "untrusted comment: …", and Tauri stores that file
/// base64-encoded, so this is how a real key begins. The placeholder in tauri.conf.json does not.
const MINISIGN_PREFIX: &str = "dW50cnVzdGVkIGNvbW1lbnQ6";

/// True when `plugins.updater` holds a real key and no placeholder endpoint.
fn config_is_real(config: &serde_json::Value) -> bool {
    let key = config.get("pubkey").and_then(|key| key.as_str()).is_some_and(|key| key.starts_with(MINISIGN_PREFIX));
    let endpoints = config
        .get("endpoints")
        .and_then(|list| list.as_array())
        .is_some_and(|list| !list.is_empty() && list.iter().all(|url| url.as_str().is_some_and(|url| !url.contains("REPLACE"))));
    key && endpoints
}

pub fn configured<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.config().plugins.0.get("updater").is_some_and(config_is_real)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    auto_check: bool,
    /// Unix seconds of the last check that reached the server, however it ended.
    last_check: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_check: true, last_check: None }
    }
}

/// True once a day has passed since `since`.
fn interval_passed(since: u64, now: u64) -> bool {
    // A timestamp in the future means the clock was set back; waiting that out would silence checks.
    since > now || now - since >= CHECK_INTERVAL_SECS
}

impl Settings {
    fn is_due(&self, now: u64) -> bool {
        self.auto_check && self.last_check.is_none_or(|last| interval_passed(last, now))
    }
}

/// What this session has done with an offer. Never saved, so a relaunch starts clean.
#[derive(Default)]
struct Offer {
    /// The sheet is up and the user hasn't answered it.
    open: bool,
    /// The user chose Download and Install and it hasn't failed yet.
    installing: bool,
    /// Unix seconds of the last "Remind Me Later" or closing of the sheet.
    dismissed_at: Option<u64>,
}

impl Offer {
    /// An unanswered offer is never stacked under another, and a dismissed one is left alone for a
    /// full interval even if the check that found it ran longer ago than that.
    fn allows_check(&self, now: u64) -> bool {
        !self.open && self.dismissed_at.is_none_or(|at| interval_passed(at, now))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs())
}

pub struct UpdatesState {
    path: PathBuf,
    settings: Mutex<Settings>,
    /// The update the sheet is showing, kept until the user decides.
    pending: Mutex<Option<Update>>,
    offer: Mutex<Offer>,
}

impl UpdatesState {
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, settings: Mutex::new(settings), pending: Mutex::new(None), offer: Mutex::new(Offer::default()) }
    }

    /// Whether a check the user didn't ask for may run now.
    fn is_due(&self, now: u64) -> bool {
        lock(&self.settings).is_due(now) && lock(&self.offer).allows_check(now)
    }

    fn installing(&self) -> bool {
        lock(&self.offer).installing
    }

    /// The update the sheet should be showing. A window that opened after the check finished, or
    /// reloaded, never saw the event announcing it, so it asks for this instead.
    fn open_offer(&self) -> Option<UpdateInfo> {
        if !lock(&self.offer).open {
            return None;
        }
        lock(&self.pending).as_ref().map(UpdateInfo::from)
    }

    fn offer_dismissed(&self, now: u64) {
        let mut offer = lock(&self.offer);
        offer.open = false;
        offer.dismissed_at = Some(now);
    }

    fn save(&self) -> CommandResult<()> {
        let json = serde_json::to_vec_pretty(&*lock(&self.settings)).map_err(|e| CommandError::from(e.to_string()))?;
        std::fs::write(&self.path, json).map_err(|e| CommandError::from(e.to_string()))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    /// Plain text from the release; GitHub's generated notes arrive as Markdown.
    notes: Option<String>,
}

impl From<&Update> for UpdateInfo {
    fn from(update: &Update) -> Self {
        Self {
            version: update.version.clone(),
            current_version: update.current_version.clone(),
            notes: update.body.clone().filter(|notes| !notes.trim().is_empty()),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckResult {
    /// A check the user asked for; those also report "up to date" and failures.
    manual: bool,
    update: Option<UpdateInfo>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesStatus {
    configured: bool,
    auto_check: bool,
    current_version: String,
    /// The offer still waiting for an answer, if any.
    update: Option<UpdateInfo>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum InstallEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        content_length: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        chunk_length: usize,
    },
    Finished,
}

async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<Option<UpdateInfo>, String> {
    let mut update =
        app.updater_builder().timeout(CHECK_TIMEOUT).build().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    if let Some(update) = &mut update {
        // The plugin documents the builder's timeout as covering downloads too, which would cut a slow
        // one off. 2.14 builds `Update` without it, but other 2.x versions may not.
        update.timeout = None;
    }
    let state = app.state::<UpdatesState>();
    lock(&state.settings).last_check = Some(now());
    // A failed write only means the next launch checks again.
    let _ = state.save();
    let info = update.as_ref().map(UpdateInfo::from);
    // Held across the writes so an install starting now sees either all of them or none.
    let mut offer = lock(&state.offer);
    if offer.installing {
        // The user clicked Download and Install while the request was out; refilling `pending`
        // would bring the buttons back after a reload and allow a second install.
        return Err("An update is already being installed".to_owned());
    }
    *lock(&state.pending) = update;
    offer.open |= info.is_some();
    Ok(info)
}

/// Checks in the background and reports to the UI through `RESULT_EVENT`.
pub fn run_check<R: Runtime>(app: &AppHandle<R>, manual: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Its sheet is mid-download; a fresh offer would refill `pending` and bring the buttons back.
        if app.state::<UpdatesState>().installing() {
            return;
        }
        let result = match check(&app).await {
            Ok(update) => CheckResult { manual, update, error: None },
            Err(error) => CheckResult { manual, update: None, error: Some(error) },
        };
        let _ = app.emit(RESULT_EVENT, result);
    });
}

/// Checks at launch and then for the life of the app, so one left open for days still hears about
/// updates. Every pass goes through the same gate: only with a real key, only if the user allows it
/// (read each time, so switching it off needs no restart), at most once a day, and never over an
/// offer that is still showing or was just dismissed.
pub fn watch<R: Runtime>(app: AppHandle<R>) {
    if !configured(&app) {
        return;
    }
    std::thread::spawn(move || {
        loop {
            if app.state::<UpdatesState>().is_due(now()) {
                run_check(&app, false);
            }
            std::thread::sleep(WATCH_INTERVAL);
        }
    });
}

#[tauri::command]
pub fn updates_status(app: AppHandle, state: State<'_, UpdatesState>) -> UpdatesStatus {
    UpdatesStatus {
        configured: configured(&app),
        auto_check: lock(&state.settings).auto_check,
        current_version: app.package_info().version.to_string(),
        update: state.open_offer(),
    }
}

#[tauri::command]
pub fn updates_set_auto_check(state: State<'_, UpdatesState>, enabled: bool) -> CommandResult<()> {
    lock(&state.settings).auto_check = enabled;
    state.save()
}

/// The user waved the offer away; background checks leave it alone for another interval.
#[tauri::command]
pub fn updates_dismiss(state: State<'_, UpdatesState>) {
    state.offer_dismissed(now());
}

/// Downloads and installs the update the sheet offered, then restarts into it.
#[tauri::command]
pub async fn updates_install(app: AppHandle, state: State<'_, UpdatesState>, on_event: Channel<InstallEvent>) -> CommandResult<()> {
    let update = {
        let mut offer = lock(&state.offer);
        let update = lock(&state.pending).take().ok_or_else(|| CommandError::from("There is no update to install".to_owned()))?;
        offer.installing = true;
        update
    };
    let mut started = false;
    let result = update
        .download_and_install(
            |chunk_length, content_length| {
                if !started {
                    started = true;
                    let _ = on_event.send(InstallEvent::Started { content_length });
                }
                let _ = on_event.send(InstallEvent::Progress { chunk_length });
            },
            || {
                let _ = on_event.send(InstallEvent::Finished);
            },
        )
        .await;
    if let Err(error) = result {
        // Keep the offer so "Download and Install" can be tried again.
        *lock(&state.pending) = Some(update);
        lock(&state.offer).installing = false;
        return Err(CommandError::from(error.to_string()));
    }
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const REAL_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDEyMzQ1Njc4OTBBQkNERUYKUldTYWJjZGVm";
    const REAL_ENDPOINT: &str = "https://github.com/someone/unsub/releases/latest/download/latest.json";

    #[test]
    fn a_real_key_and_endpoint_count_as_configured() {
        assert!(config_is_real(&json!({ "pubkey": REAL_KEY, "endpoints": [REAL_ENDPOINT] })));
    }

    #[test]
    fn the_shipped_placeholders_do_not() {
        let placeholder_key = "REPLACE: run `pnpm tauri signer generate -w ~/.tauri/unsub.key` and paste the .pub file here";
        let placeholder_endpoint = "https://github.com/REPLACE_OWNER/REPLACE_REPO/releases/latest/download/latest.json";
        assert!(!config_is_real(&json!({ "pubkey": placeholder_key, "endpoints": [REAL_ENDPOINT] })));
        assert!(!config_is_real(&json!({ "pubkey": REAL_KEY, "endpoints": [placeholder_endpoint] })));
        assert!(!config_is_real(&json!({ "pubkey": REAL_KEY, "endpoints": [] })));
        assert!(!config_is_real(&json!({ "pubkey": REAL_KEY })));
        assert!(!config_is_real(&json!({ "endpoints": [REAL_ENDPOINT] })));
        assert!(!config_is_real(&json!(null)));
    }

    #[test]
    fn checks_are_due_after_a_day_and_never_when_switched_off() {
        let day = CHECK_INTERVAL_SECS;
        let settings = |auto_check, last_check| Settings { auto_check, last_check };
        assert!(settings(true, None).is_due(1_000));
        assert!(!settings(true, Some(1_000)).is_due(1_000 + day - 1));
        assert!(settings(true, Some(1_000)).is_due(1_000 + day));
        assert!(!settings(false, None).is_due(1_000 + day));
        assert!(!settings(true, Some(1_000)).is_due(1_000 + WATCH_INTERVAL.as_secs()), "an hourly pass must not check hourly");
        assert!(settings(true, Some(5_000)).is_due(1_000), "a clock set back must not silence checks");
    }

    fn state_with(auto_check: bool, last_check: Option<u64>) -> UpdatesState {
        let state = UpdatesState::load(PathBuf::from("/nonexistent/Updates.json"));
        *lock(&state.settings) = Settings { auto_check, last_check };
        state
    }

    #[test]
    fn an_open_offer_is_never_stacked() {
        let day = CHECK_INTERVAL_SECS;
        let state = state_with(true, Some(1_000));
        lock(&state.offer).open = true;
        assert!(!state.is_due(1_000 + 3 * day));
    }

    #[test]
    fn a_dismissed_offer_waits_a_full_interval_from_the_dismissal() {
        let day = CHECK_INTERVAL_SECS;
        // Found at 1_000, left open for almost a day; the check itself is already due again.
        let state = state_with(true, Some(1_000));
        lock(&state.offer).open = true;
        let dismissed_at = 1_000 + day - 60;
        state.offer_dismissed(dismissed_at);
        assert!(!state.is_due(1_000 + day));
        assert!(!state.is_due(dismissed_at + day - 1));
        assert!(state.is_due(dismissed_at + day));
        assert!(lock(&state.offer).allows_check(dismissed_at - 1), "a clock set back must not silence checks");
    }

    #[test]
    fn an_offer_is_reported_only_while_the_sheet_waits_on_one() {
        let state = state_with(true, None);
        assert!(state.open_offer().is_none());
        // Open with nothing pending is what an install in progress looks like.
        lock(&state.offer).open = true;
        assert!(state.open_offer().is_none());
    }

    #[test]
    fn settings_default_to_checking_and_survive_a_missing_file() {
        let state = UpdatesState::load(PathBuf::from("/nonexistent/Updates.json"));
        assert!(lock(&state.settings).auto_check);
        let partial: Settings = serde_json::from_str(r#"{"lastCheck": 7}"#).unwrap();
        assert!(partial.auto_check);
        assert_eq!(partial.last_check, Some(7));
    }
}
