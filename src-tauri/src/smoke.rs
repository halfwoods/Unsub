//! Opt-in package validation. Validation runs before opening any application state.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{Listener, Manager};

#[derive(Clone, Debug)]
pub struct Config {
    fixture: PathBuf,
    report: PathBuf,
    home: PathBuf,
}
#[derive(Default)]
pub struct Ready(AtomicBool);

fn child(path: &Path, root: &Path) -> bool {
    path != root && path.starts_with(root)
}

pub fn config() -> Result<Option<Config>, String> {
    parse(std::env::args().skip(1).collect(), |key| std::env::var_os(key).map(PathBuf::from), cfg!(target_os = "linux"))
}

fn parse(args: Vec<String>, env: impl Fn(&str) -> Option<PathBuf>, linux: bool) -> Result<Option<Config>, String> {
    let requested = args.iter().any(|s| s == "--smoke-test" || s == "--smoke-report") || env("UNSUB_SMOKE_TEST").is_some();
    if !requested {
        return Ok(None);
    }
    if !linux || env("UNSUB_SMOKE_TEST") != Some(PathBuf::from("1")) {
        return Err("Smoke mode requires Linux and UNSUB_SMOKE_TEST=1".into());
    }
    if args.len() != 4 || args[0] != "--smoke-test" || args[2] != "--smoke-report" {
        return Err("Expected --smoke-test FIXTURE --smoke-report REPORT".into());
    }
    let canonical = |path: &Path| path.canonicalize().map_err(|error| format!("{}: {error}", path.display()));
    let home = canonical(&env("HOME").ok_or("Smoke HOME is required")?)?;
    let temp = canonical(&std::env::temp_dir())?;
    let tmp = Path::new("/tmp").canonicalize().ok();
    if !child(&home, &temp) && !tmp.as_ref().is_some_and(|tmp| child(&home, tmp)) {
        return Err("Smoke HOME must be a disposable directory below the system temp directory".into());
    }
    for name in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR"] {
        let directory = canonical(&env(name).ok_or_else(|| format!("{name} is required"))?)?;
        if !directory.is_dir() || !child(&directory, &home) {
            return Err(format!("{name} must be a directory below smoke HOME"));
        }
        // Tauri/WebKit can populate the app directory before the setup hook runs.
        // Reject existing data here, before constructing the builder or webview.
        if name == "XDG_DATA_HOME" && std::fs::read_dir(&directory).map_err(|e| e.to_string())?.next().is_some() {
            return Err("Smoke XDG_DATA_HOME must be empty before startup".into());
        }
    }
    let fixture = canonical(Path::new(&args[1]))?;
    let report = PathBuf::from(&args[3]);
    let parent = canonical(report.parent().ok_or("Report needs a parent directory")?)?;
    if !fixture.is_file()
        || !child(&fixture, &home)
        || (parent != home && !child(&parent, &home))
        || report.symlink_metadata().is_ok()
        || report.file_name().is_none()
    {
        return Err("Fixture and new report must be inside smoke HOME".into());
    }
    Ok(Some(Config { fixture, report: parent.join(report.file_name().unwrap()), home }))
}

impl Config {
    pub fn check_path(&self, path: &Path) -> Result<(), String> {
        // Check the nearest existing ancestor before creating anything.
        if !child(path, &self.home) || path.components().any(|part| part == std::path::Component::ParentDir) {
            return Err("Application directory escaped smoke HOME".into());
        }
        let ancestor = path.ancestors().find(|part| part.exists()).ok_or("Application directory has no ancestor")?;
        let resolved = ancestor.canonicalize().map_err(|e| e.to_string())?;
        if resolved != self.home && !child(&resolved, &self.home) {
            return Err("Application directory symlink escaped smoke HOME".into());
        }
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        if !child(&path.canonicalize().map_err(|e| e.to_string())?, &self.home) {
            return Err("Application directory escaped smoke HOME".into());
        }
        Ok(())
    }
}

/// Called by a mounted React tree; normal launches have no smoke Ready state.
#[tauri::command]
pub fn smoke_frontend_ready(app: tauri::AppHandle) {
    if let Some(ready) = app.try_state::<Ready>() {
        ready.0.store(true, Ordering::Release);
    }
}

pub fn start(app: tauri::AppHandle, config: Config) {
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = std::sync::Arc::clone(&events);
    let listener = app.listen(crate::playback::PLAYER_EVENT, move |event| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload())
            && matches!(value.get("type").and_then(|kind| kind.as_str()), Some("error" | "notice"))
        {
            let mut events = captured.lock().unwrap();
            if events.len() < 32 {
                events.push(value);
            }
        }
    });
    std::thread::spawn(move || {
        let result = exercise(&app, &config);
        app.unlisten(listener);
        let success = result.is_ok();
        let report = match result {
            Ok(status) => serde_json::json!({"success":true,"frontendReady":true,"imported":true,"playback":status}),
            Err(error) => serde_json::json!({"success":false,"error":error,"playbackEvents":events.lock().unwrap().clone()}),
        };
        let written = write_report(&config.report, &report);
        if let Err(error) = &written {
            eprintln!("Smoke report: {error}");
        }
        app.exit(if success && written.is_ok() { 0 } else { 1 });
    });
}

fn write_report(path: &Path, report: &serde_json::Value) -> std::io::Result<()> {
    // Never truncate a file or follow a symlink introduced since argument validation.
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(&file, report).map_err(std::io::Error::other)?;
    file.sync_all()
}

fn exercise(app: &tauri::AppHandle, config: &Config) -> Result<unsub_playback::PlaybackStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(50);
    while !app.state::<Ready>().0.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            return Err("React frontend did not become ready".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let library = app.state::<crate::library::LibraryState>();
    library.with_writer(|library| library.add_paths(std::slice::from_ref(&config.fixture), |_| {})).map_err(|e| e.to_string())?;
    let tracks = library.queries().tracks(&Default::default()).map_err(|e| e.to_string())?;
    if tracks.len() != 1 {
        return Err(format!("Expected one imported fixture, found {} tracks", tracks.len()));
    }
    let id = tracks[0].id;
    crate::playback::player_play_list(
        app.clone(),
        app.state(),
        app.state(),
        app.state(),
        crate::playback::PlaySource::Library,
        vec![id],
        0,
    )
    .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut first_position = None;
    loop {
        let status = crate::playback::player_status(app.state());
        if status.state == unsub_playback::PlaybackState::Playing
            && status.track_id == Some(id)
            && status.output_sample_rate.unwrap_or(0) > 0
        {
            let initial = *first_position.get_or_insert(status.position_ms);
            if status.position_ms >= initial + 250 {
                return Ok(status);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("Fixture did not advance through an audio output: {status:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_launch_and_invalid_opt_in() {
        assert!(parse(vec![], |_| None, true).unwrap().is_none());
        assert!(parse(vec!["--smoke-test".into()], |_| None, true).is_err());
        assert!(parse(vec![], |_| Some("1".into()), false).is_err());
    }
    #[test]
    fn smoke_requires_isolated_directories_fixture_and_new_report() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap();
        let fixture = home.join("fixture.wav");
        std::fs::write(&fixture, b"fixture").unwrap();
        let mut env = std::collections::HashMap::from([("HOME", home.clone()), ("UNSUB_SMOKE_TEST", PathBuf::from("1"))]);
        for name in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR"] {
            let directory = home.join(name);
            std::fs::create_dir(&directory).unwrap();
            env.insert(name, directory);
        }
        let report = home.join("report.json");
        let args = vec!["--smoke-test".into(), fixture.display().to_string(), "--smoke-report".into(), report.display().to_string()];
        let config = parse(args.clone(), |key| env.get(key).cloned(), true).unwrap().unwrap();
        let existing_data = home.join("XDG_DATA_HOME/existing-library");
        std::fs::write(&existing_data, b"existing").unwrap();
        assert!(parse(args.clone(), |key| env.get(key).cloned(), true).unwrap_err().contains("must be empty"));
        std::fs::remove_file(existing_data).unwrap();
        // Webview-created state after the early guard must not fail setup.
        let app_data = home.join("XDG_DATA_HOME/unsub");
        std::fs::create_dir_all(app_data.join("webkit")).unwrap();
        assert!(config.check_path(&app_data).is_ok());
        std::fs::remove_dir_all(app_data).unwrap();
        assert!(config.check_path(&home.join("new/cache")).is_ok());
        assert!(config.check_path(&home.join("../escaped-smoke-test")).is_err());
        env.insert("XDG_CACHE_HOME", std::env::temp_dir());
        assert!(parse(args.clone(), |key| env.get(key).cloned(), true).is_err());
        env.insert("XDG_CACHE_HOME", home.join("XDG_CACHE_HOME"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.join("absent"), &report).unwrap();
            assert!(parse(args.clone(), |key| env.get(key).cloned(), true).is_err());
            std::fs::remove_file(&report).unwrap();
        }
        std::fs::write(&report, b"existing").unwrap();
        assert!(parse(args, |key| env.get(key).cloned(), true).is_err());
    }
    #[test]
    fn report_creation_never_overwrites_an_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let report = root.path().join("report.json");
        let result = serde_json::json!({"success":true});
        write_report(&report, &result).unwrap();
        let original = std::fs::read(&report).unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&original).unwrap(), result);
        assert_eq!(write_report(&report, &serde_json::json!({"success":false})).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(report).unwrap(), original);
    }
    #[cfg(unix)]
    #[test]
    fn report_creation_rejects_a_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("must-not-be-created");
        let report = root.path().join("report.json");
        std::os::unix::fs::symlink(&target, &report).unwrap();
        assert!(write_report(&report, &serde_json::json!({"success":true})).is_err());
        assert!(!target.exists());
    }
    #[cfg(unix)]
    #[test]
    fn application_paths_cannot_follow_symlinks_outside_profile() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("escape")).unwrap();
        let config =
            Config { home: home.path().canonicalize().unwrap(), fixture: home.path().join("fixture"), report: home.path().join("report") };
        assert!(config.check_path(&home.path().canonicalize().unwrap().join("escape/new")).is_err());
        assert!(!outside.path().join("new").exists());
    }
    #[test]
    fn isolation_uses_path_components() {
        assert!(child(Path::new("/tmp/profile/cache"), Path::new("/tmp/profile")));
        assert!(!child(Path::new("/tmp/profile-other"), Path::new("/tmp/profile")));
        assert!(!child(Path::new("/tmp/profile"), Path::new("/tmp/profile")));
    }
}
