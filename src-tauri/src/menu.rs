use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_dialog::DialogExt;

use crate::library;

/// Controls menu items are handled by the UI, which owns "what Play means" (selection vs queue).
pub const MENU_ACTION_EVENT: &str = "menu://action";

const ADD_TO_LIBRARY: &str = "add-to-library";
const UI_ACTIONS: [(&str, &str, Option<&str>); 5] = [
    ("play-pause", "Play/Pause", None),
    ("next", "Next", Some("CmdOrCtrl+Right")),
    ("previous", "Previous", Some("CmdOrCtrl+Left")),
    ("volume-up", "Increase Volume", Some("CmdOrCtrl+Up")),
    ("volume-down", "Decrease Volume", Some("CmdOrCtrl+Down")),
];

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let app_menu = Submenu::with_items(
        app,
        "AntiStreaming",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About AntiStreaming"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, ADD_TO_LIBRARY, "Add to Library…", true, Some("CmdOrCtrl+O"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    // Predefined edit items keep copy/paste and undo working in the search field.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let controls = Submenu::new(app, "Controls", true)?;
    for (index, (id, label, accelerator)) in UI_ACTIONS.into_iter().enumerate() {
        if index == 3 {
            controls.append(&PredefinedMenuItem::separator(app)?)?;
        }
        controls.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[&PredefinedMenuItem::minimize(app, None)?, &PredefinedMenuItem::maximize(app, None)?],
    )?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &controls, &window])
}

pub fn on_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    if id == ADD_TO_LIBRARY {
        let handle = app.clone();
        app.dialog().file().set_title("Add to Library").pick_folders(move |folders| {
            let paths: Vec<_> = folders.unwrap_or_default().into_iter().filter_map(|f| f.into_path().ok()).collect();
            if !paths.is_empty() {
                library::add_paths(&handle, paths);
            }
        });
    } else if UI_ACTIONS.iter().any(|(action, _, _)| *action == id) {
        let _ = app.emit(MENU_ACTION_EVENT, id);
    }
}
