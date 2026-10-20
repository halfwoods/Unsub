//! Updates through tauri-plugin-updater: a check at launch (at most once a day), "Check for
//! Updates…" in the app menu, and the download and install behind the UI's update sheet.
//!
//! Nothing here contacts anyone unless tauri.conf.json carries a real public key and endpoint
//! (`configured`), so a build made without the maintainer's signing key never offers updates and
//! never fails because of that.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{CommandError, CommandResult};

/// The UI listens to this for the outcome of every check.
pub const RESULT_EVENT: &str = "updates://result";

const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

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

impl Settings {
    fn is_due(&self, now: u64) -> bool {
        // A last check in the future means the clock was set back; waiting that out would silence checks.
        self.auto_check && self.last_check.is_none_or(|last| last > now || now - last >= CHECK_INTERVAL_SECS)
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
}

impl UpdatesState {
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { path, settings: Mutex::new(settings), pending: Mutex::new(None) }
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
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let state = app.state::<UpdatesState>();
    lock(&state.settings).last_check = Some(now());
    // A failed write only means the next launch checks again.
    let _ = state.save();
    let info = update.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone().filter(|notes| !notes.trim().is_empty()),
    });
    *lock(&state.pending) = update;
    Ok(info)
}

/// Checks in the background and reports to the UI through `RESULT_EVENT`.
pub fn run_check<R: Runtime>(app: &AppHandle<R>, manual: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = match check(&app).await {
            Ok(update) => CheckResult { manual, update, error: None },
            Err(error) => CheckResult { manual, update: None, error: Some(error) },
        };
        let _ = app.emit(RESULT_EVENT, result);
    });
}

/// The launch-time check: only with a real key, only if the user allows it, at most once a day.
pub fn check_on_launch<R: Runtime>(app: &AppHandle<R>) {
    if configured(app) && lock(&app.state::<UpdatesState>().settings).is_due(now()) {
        run_check(app, false);
    }
}

#[tauri::command]
pub fn updates_status(app: AppHandle, state: State<'_, UpdatesState>) -> UpdatesStatus {
    UpdatesStatus {
        configured: configured(&app),
        auto_check: lock(&state.settings).auto_check,
        current_version: app.package_info().version.to_string(),
    }
}

#[tauri::command]
pub fn updates_set_auto_check(state: State<'_, UpdatesState>, enabled: bool) -> CommandResult<()> {
    lock(&state.settings).auto_check = enabled;
    state.save()
}

/// Downloads and installs the update the sheet offered, then restarts into it.
#[tauri::command]
pub async fn updates_install(app: AppHandle, state: State<'_, UpdatesState>, on_event: Channel<InstallEvent>) -> CommandResult<()> {
    let update = lock(&state.pending).take().ok_or_else(|| CommandError::from("There is no update to install".to_owned()))?;
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
        assert!(settings(true, Some(5_000)).is_due(1_000), "a clock set back must not silence checks");
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
