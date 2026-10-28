//! The main window beyond what a frameless Tauri window gives: what closing it means, getting it back,
//! full screen, and what the system wants a double-click on its top edge to do.

use serde::Serialize;
use tauri::{AppHandle, Manager, Window, WindowEvent};

use crate::menu;

const MAIN: &str = "main";

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

pub fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.show();
        // A minimized window doesn't count as visible, so a click on the Dock icon can mean this too.
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn toggle_full_screen(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.set_fullscreen(!window.is_fullscreen().unwrap_or(false));
    }
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
