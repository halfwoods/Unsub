//! The main window beyond what a frameless Tauri window gives: what closing it means, getting it back,
//! full screen, the mini player, and what the system wants a double-click on its top edge to do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Window, WindowEvent};

use crate::menu;
use crate::window_state::{self, Layout, MINI_HEIGHT, MINI_MAX_WIDTH, MINI_MIN_WIDTH};

const MAIN: &str = "main";
/// The window is the same one in both modes, so the UI is told when it changes shape rather than assuming it did.
pub const MINI_PLAYER_EVENT: &str = "window://mini-player";
/// Leaving full screen animates, and a size set during it is lost.
const FULL_SCREEN_EXIT_WAIT: Duration = Duration::from_secs(3);
const FULL_SCREEN_SETTLED: Duration = Duration::from_millis(300);
/// Held for a whole switch. Only threads off the main one take it, since a switch asks the main thread for answers.
static SWITCHING: Mutex<()> = Mutex::new(());
static MINI_ALLOWED: AtomicBool = AtomicBool::new(true);

/// What a double-click on the toolbar does, as set for title bars in the system settings.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DoubleClickAction {
    Zoom,
    Minimize,
    Nothing,
}

/// Read at each double-click so a change in System Settings applies without a restart.
#[tauri::command]
pub fn window_double_click_action() -> DoubleClickAction {
    double_click_action(system_setting().as_deref())
}

/// A missing or unknown setting zooms, as the system does.
fn double_click_action(setting: Option<&str>) -> DoubleClickAction {
    match setting {
        Some("Minimize") => DoubleClickAction::Minimize,
        Some("None") => DoubleClickAction::Nothing,
        _ => DoubleClickAction::Zoom,
    }
}

#[cfg(target_os = "macos")]
fn system_setting() -> Option<String> {
    use objc2_foundation::{NSUserDefaults, ns_string};

    NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleActionOnDoubleClick")).map(|setting| setting.to_string())
}

#[cfg(not(target_os = "macos"))]
fn system_setting() -> Option<String> {
    None
}

/// AppKit adds its own "Enter Full Screen" to a menu titled View unless told not to, and it would sit beside ours.
/// It reads this as the app starts, so this runs before anything else does.
#[cfg(target_os = "macos")]
pub fn keep_full_screen_item_ours() {
    use objc2_foundation::{NSUserDefaults, ns_string};

    NSUserDefaults::standardUserDefaults().setBool_forKey(false, ns_string!("NSFullScreenMenuItemEverywhere"));
}

/// Whether the window is the mini player now, for the UI to start from.
#[tauri::command]
pub fn window_mini_player(app: AppHandle) -> bool {
    window_state::is_mini(&app)
}

/// Blocks while full screen is left, so it runs off the threads that the window system itself needs.
#[tauri::command]
pub async fn set_mini_player(app: AppHandle, enabled: bool) {
    let _ = tauri::async_runtime::spawn_blocking(move || set_mini(&app, enabled)).await;
}

/// The UI knows what is open (sheets keep their drafts in their own state), and the player can't be entered over any of it.
#[tauri::command]
pub fn set_mini_player_allowed(app: AppHandle, allowed: bool) {
    MINI_ALLOWED.store(allowed, Ordering::SeqCst);
    sync_menu(&app);
}

/// The Window menu item follows the mode and whether it can be entered; leaving is always possible.
pub fn sync_menu(app: &AppHandle) {
    menu::sync_mini_player_item(app, window_state::is_mini(app), MINI_ALLOWED.load(Ordering::SeqCst));
}

pub fn toggle_mini_player(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        set_mini(&app, !window_state::is_mini(&app));
        // Switching to the mini player from a closed window is a request to see it.
        reveal(&app);
    });
}

/// Turns the main window into the mini player, or back, one switch at a time: the saved mode, the frame, the menu
/// and the event all describe the same switch. Entering takes the window out of full screen first.
fn set_mini(app: &AppHandle, enabled: bool) {
    let _switching = SWITCHING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(window) = app.get_webview_window(MAIN) else { return };
    let window = window.as_ref().window();
    if enabled {
        if !MINI_ALLOWED.load(Ordering::SeqCst) {
            return;
        }
        leave_full_screen(&window);
    }
    if let Some((layout, minimum)) = window_state::switch_mini(app, enabled) {
        apply_layout(&window, layout, minimum);
    }
    sync_menu(app);
    let _ = app.emit(MINI_PLAYER_EVENT, window_state::is_mini(app));
}

/// Waits out the system's animation. The window reports it is no longer full screen as the animation starts,
/// and a size set meanwhile is overwritten when it ends, so the frame has to stop changing as well.
fn leave_full_screen(window: &Window) {
    if !window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let _ = window.set_fullscreen(false);
    let deadline = Instant::now() + FULL_SCREEN_EXIT_WAIT;
    let mut frame = None;
    let mut still_since = Instant::now();
    while Instant::now() < deadline {
        let now = (window.outer_position().ok(), window.outer_size().ok());
        if now != frame {
            frame = now;
            still_since = Instant::now();
        }
        if !window.is_fullscreen().unwrap_or(false) && still_since.elapsed() >= FULL_SCREEN_SETTLED {
            return;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Un-maximizing comes first on the way in, since the limits below can't hold a zoomed window to a bar.
pub fn apply_layout(window: &Window, layout: Layout, normal_minimum: LogicalSize<f64>) {
    match layout {
        Layout::Mini { width } => {
            let _ = window.unmaximize();
            let _ = window.set_min_size(Some(LogicalSize::new(MINI_MIN_WIDTH, MINI_HEIGHT)));
            let _ = window.set_max_size(Some(LogicalSize::new(MINI_MAX_WIDTH, MINI_HEIGHT)));
            let _ = window.set_size(LogicalSize::new(width, MINI_HEIGHT));
        }
        Layout::Normal { size, maximized } => {
            let _ = window.set_max_size(None::<LogicalSize<f64>>);
            let _ = window.set_min_size(Some(normal_minimum));
            let _ = window.set_size(size);
            if maximized {
                let _ = window.maximize();
            }
        }
    }
}

pub fn on_event(window: &Window, event: &WindowEvent) {
    crate::window_state::on_event(window, event);
    match event {
        // The player outlives its window on macOS: music plays on, the Dock icon or the Window menu brings the window back,
        // and only Quit quits. Quitting doesn't go through here, so what runs on exit still runs.
        #[cfg(target_os = "macos")]
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            // A hidden full-screen window leaves a blank Space behind, so the first close only leaves full screen.
            if window.is_fullscreen().unwrap_or(false) {
                let _ = window.set_fullscreen(false);
            } else {
                let _ = window.hide();
            }
        }
        WindowEvent::Resized(_) => menu::sync_full_screen_label(window),
        _ => {}
    }
}

/// Does what the close button does, which a borderless window can't be asked for by the system's own Close Window.
#[cfg(target_os = "macos")]
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.close();
    }
}

/// Like the minimize button, which a full-screen window leaves disabled. A hidden or minimized
/// window has no button to press, so the menu item does nothing then, as the system's own does.
#[cfg(target_os = "macos")]
pub fn minimize(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN)
        && window.is_visible().unwrap_or(false)
        && !window.is_fullscreen().unwrap_or(false)
    {
        let _ = window.minimize();
    }
}

/// Like the zoom button, which leaves full screen instead when there is one. Not while the window
/// is hidden or minimized, which would only reopen it resized.
#[cfg(target_os = "macos")]
pub fn zoom(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN)
        && window.is_visible().unwrap_or(false)
        && !window_state::is_mini(app)
    {
        let _ = if window.is_fullscreen().unwrap_or(false) {
            window.set_fullscreen(false)
        } else if window.is_maximized().unwrap_or(false) {
            window.unmaximize()
        } else {
            window.maximize()
        };
    }
}

/// Brings the window back as the full one: what asks for it (the Dock icon, a menu item with a sheet to show) has no place in the mini player.
pub fn show(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        set_mini(&app, false);
        reveal(&app);
    });
}

fn reveal(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.show();
        // A minimized window doesn't count as visible, so a click on the Dock icon can mean this too.
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn toggle_full_screen(app: &AppHandle) {
    let app = app.clone();
    // After the switch back from the mini player, which has no full screen to go to.
    tauri::async_runtime::spawn_blocking(move || {
        set_mini(&app, false);
        reveal(&app);
        if let Some(window) = app.get_webview_window(MAIN) {
            let _ = window.set_fullscreen(!window.is_fullscreen().unwrap_or(false));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_systems_choice_for_a_title_bar_double_click_is_followed() {
        assert_eq!(double_click_action(Some("Maximize")), DoubleClickAction::Zoom);
        assert_eq!(double_click_action(Some("Minimize")), DoubleClickAction::Minimize);
        assert_eq!(double_click_action(Some("None")), DoubleClickAction::Nothing);
    }

    #[test]
    fn a_double_click_zooms_when_the_setting_is_missing_or_unknown() {
        assert_eq!(double_click_action(None), DoubleClickAction::Zoom);
        assert_eq!(double_click_action(Some("Fill")), DoubleClickAction::Zoom);
    }
}
