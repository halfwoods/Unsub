use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_dialog::DialogExt;

use crate::{import, library};

/// Playlist and Controls items are handled by the UI, which owns the selection and "what Play means".
pub const MENU_ACTION_EVENT: &str = "menu://action";

const ADD_TO_LIBRARY: &str = "add-to-library";
const IMPORT_MUSIC: &str = "import-music-library";
const IMPORT_XML: &str = "import-library-xml";
const FILE_ACTIONS: [(&str, &str, Option<&str>); 4] = [
    ("new-playlist", "New Playlist", Some("CmdOrCtrl+N")),
    ("new-playlist-from-selection", "New Playlist from Selection", Some("Shift+CmdOrCtrl+N")),
    ("new-smart-playlist", "New Smart Playlist…", Some("Alt+CmdOrCtrl+N")),
    ("new-playlist-folder", "New Playlist Folder", Some("Shift+Alt+CmdOrCtrl+N")),
];
const CONTROL_ACTIONS: [(&str, &str, Option<&str>); 5] = [
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
    let file = Submenu::new(app, "File", true)?;
    for (id, label, accelerator) in FILE_ACTIONS {
        file.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    file.append(&PredefinedMenuItem::separator(app)?)?;
    file.append(&MenuItem::with_id(app, ADD_TO_LIBRARY, "Add to Library…", true, Some("CmdOrCtrl+O"))?)?;
    #[cfg(target_os = "macos")]
    file.append(&MenuItem::with_id(app, IMPORT_MUSIC, "Import Music Library", true, None::<&str>)?)?;
    file.append(&MenuItem::with_id(app, IMPORT_XML, "Import Library XML…", true, None::<&str>)?)?;
    file.append(&PredefinedMenuItem::separator(app)?)?;
    file.append(&PredefinedMenuItem::close_window(app, None)?)?;
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
    for (index, (id, label, accelerator)) in CONTROL_ACTIONS.into_iter().enumerate() {
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
    } else if id == IMPORT_MUSIC {
        import::run(app, None);
    } else if id == IMPORT_XML {
        let handle = app.clone();
        app.dialog().file().set_title("Import Library XML").add_filter("Library XML", &["xml"]).pick_file(move |file| {
            if let Some(path) = file.and_then(|f| f.into_path().ok()) {
                import::run(&handle, Some(path));
            }
        });
    } else if FILE_ACTIONS.iter().chain(&CONTROL_ACTIONS).any(|(action, _, _)| *action == id) {
        let _ = app.emit(MENU_ACTION_EVENT, id);
    }
}
