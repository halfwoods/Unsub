//! Remember the main window's normal size, including when Quit happens in full screen or minimized.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalSize, Manager, Window, WindowEvent};

const MAIN: &str = "main";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
struct SavedWindow {
    width: f64,
    height: f64,
    #[serde(default)]
    maximized: bool,
}

impl SavedWindow {
    fn load(path: &Path) -> Option<Self> {
        let saved: Self = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        saved.valid().then_some(saved)
    }

    fn valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }

    fn fit(mut self, minimum: LogicalSize<f64>, maximum: Option<LogicalSize<f64>>) -> Self {
        let maximum = maximum.unwrap_or(LogicalSize::new(f64::MAX, f64::MAX));
        self.width = self.width.clamp(minimum.width, maximum.width.max(minimum.width));
        self.height = self.height.clamp(minimum.height, maximum.height.max(minimum.height));
        self
    }

    fn observe(&mut self, size: LogicalSize<f64>, maximized: bool, fullscreen: bool, minimized: bool) {
        // Full screen is temporary (the visualizer uses it too), and a minimized window can report zero size.
        if fullscreen || minimized {
            return;
        }
        let normal = Self { width: size.width, height: size.height, maximized };
        if !normal.valid() {
            return;
        }
        self.maximized = maximized;
        if !maximized {
            *self = normal;
        }
    }
}

struct WindowState {
    path: PathBuf,
    saved: Mutex<SavedWindow>,
}

/// Run after data migration and before the user can resize the already-created main window.
pub fn restore(app: &AppHandle, path: PathBuf) {
    let Some(webview) = app.get_webview_window(MAIN) else { return };
    let window = webview.as_ref().window();
    let Ok(size) = window.inner_size().and_then(|size| window.scale_factor().map(|scale| size.to_logical::<f64>(scale))) else { return };
    let stored = SavedWindow::load(&path);
    let config = app.config().app.windows.iter().find(|window| window.label == MAIN);
    let minimum = LogicalSize::new(
        config.and_then(|config| config.min_width).unwrap_or(1.0),
        config.and_then(|config| config.min_height).unwrap_or(1.0),
    );
    let maximum = window.current_monitor().ok().flatten().map(|monitor| monitor.work_area().size.to_logical::<f64>(monitor.scale_factor()));
    let saved = stored.unwrap_or(SavedWindow { width: size.width, height: size.height, maximized: false }).fit(minimum, maximum);
    app.manage(WindowState { path, saved: Mutex::new(saved) });
    if stored.is_some() {
        let _ = window.set_size(LogicalSize::new(saved.width, saved.height));
        if saved.maximized {
            let _ = window.maximize();
        }
    }
}

fn record(window: &Window) {
    if window.label() != MAIN {
        return;
    }
    let Some(state) = window.try_state::<WindowState>() else { return };
    let (Ok(size), Ok(scale), Ok(maximized), Ok(fullscreen), Ok(minimized)) =
        (window.inner_size(), window.scale_factor(), window.is_maximized(), window.is_fullscreen(), window.is_minimized())
    else {
        return;
    };
    state.saved.lock().unwrap_or_else(PoisonError::into_inner).observe(size.to_logical(scale), maximized, fullscreen, minimized);
}

/// Closing destroys the main window on Windows/Linux; on macOS it only hides it.
pub fn on_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => record(window),
        WindowEvent::CloseRequested { .. } if window.label() == MAIN => save(window.app_handle()),
        _ => {}
    }
}

/// Quit does not have to close the window first, so save on ExitRequested as well.
pub fn save(app: &AppHandle) {
    let Some(state) = app.try_state::<WindowState>() else { return };
    if let Some(window) = app.get_webview_window(MAIN) {
        record(&window.as_ref().window());
    }
    let saved = *state.saved.lock().unwrap_or_else(PoisonError::into_inner);
    let written =
        serde_json::to_vec_pretty(&saved).map_err(std::io::Error::other).and_then(|bytes| crate::write_private(&state.path, &bytes));
    if let Err(error) = written {
        eprintln!("[window] couldn't save window size: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal() -> SavedWindow {
        SavedWindow { width: 950.0, height: 600.0, maximized: false }
    }

    #[test]
    fn normal_size_and_zoom_round_trip_without_physical_pixel_scaling() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Window.json");
        let mut saved = normal();
        saved.observe(tauri::PhysicalSize::new(2000, 1300).to_logical(2.0), false, false, false);
        saved.observe(LogicalSize::new(1512.0, 949.0), true, false, false);
        crate::write_private(&path, &serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(SavedWindow::load(&path), Some(SavedWindow { width: 1000.0, height: 650.0, maximized: true }));
    }

    #[test]
    fn fullscreen_minimized_and_zero_sizes_do_not_replace_the_normal_size() {
        let mut saved = normal();
        for (size, maximized, fullscreen, minimized) in [
            (LogicalSize::new(1512.0, 982.0), true, true, false),
            (LogicalSize::new(0.0, 0.0), false, false, true),
            (LogicalSize::new(0.0, 0.0), false, false, false),
        ] {
            saved.observe(size, maximized, fullscreen, minimized);
            assert_eq!(saved, normal());
        }
    }

    #[test]
    fn unmaximizing_and_resizing_remembers_the_new_normal_size() {
        let mut saved = normal();
        saved.observe(LogicalSize::new(1512.0, 949.0), true, false, false);
        assert_eq!(saved, SavedWindow { maximized: true, ..normal() });
        saved.observe(LogicalSize::new(1000.0, 650.0), false, false, false);
        assert_eq!(saved, SavedWindow { width: 1000.0, height: 650.0, maximized: false });
    }

    #[test]
    fn missing_corrupt_or_invalid_settings_fall_back_to_the_default_window() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Window.json");
        assert_eq!(SavedWindow::load(&path), None);
        for bytes in ["{bad", "null", r#"{"width":0,"height":600}"#, r#"{"width":950,"height":-1}"#] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(SavedWindow::load(&path), None);
        }
        assert!(!SavedWindow { width: f64::INFINITY, ..normal() }.valid());
        assert!(!SavedWindow { height: f64::NAN, ..normal() }.valid());
    }

    #[test]
    fn restore_size_fits_the_current_monitor_and_window_minimum() {
        let minimum = LogicalSize::new(820.0, 420.0);
        let maximum = Some(LogicalSize::new(1440.0, 900.0));
        assert_eq!(
            SavedWindow { width: 4000.0, height: 3000.0, maximized: false }.fit(minimum, maximum),
            SavedWindow { width: 1440.0, height: 900.0, maximized: false }
        );
        assert_eq!(
            SavedWindow { width: 10.0, height: 10.0, maximized: false }.fit(minimum, maximum),
            SavedWindow { width: 820.0, height: 420.0, maximized: false }
        );
    }
}
