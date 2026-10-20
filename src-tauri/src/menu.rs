use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_dialog::DialogExt;

use crate::{import, library, updates};

/// Playlist and Controls items are handled by the UI, which owns the selection and "what Play means".
pub const MENU_ACTION_EVENT: &str = "menu://action";

const ADD_TO_LIBRARY: &str = "add-to-library";
const IMPORT_MUSIC: &str = "import-music-library";
const IMPORT_XML: &str = "import-library-xml";
const CHECK_FOR_UPDATES: &str = "check-for-updates";
const PREFERENCES: (&str, &str, Option<&str>) = ("preferences", "Preferences…", Some("CmdOrCtrl+,"));
const EQUALIZER: (&str, &str, Option<&str>) = ("equalizer", "Equalizer", Some("Alt+CmdOrCtrl+E"));
const FILE_ACTIONS: [(&str, &str, Option<&str>); 5] = [
    ("new-playlist", "New Playlist", Some("CmdOrCtrl+N")),
    ("new-playlist-from-selection", "New Playlist from Selection", Some("Shift+CmdOrCtrl+N")),
    ("new-smart-playlist", "New Smart Playlist…", Some("Alt+CmdOrCtrl+N")),
    ("new-playlist-folder", "New Playlist Folder", Some("Shift+Alt+CmdOrCtrl+N")),
    ("open-stream", "Open Stream…", Some("CmdOrCtrl+U")),
];
/// The selection-dependent half of iTunes 10's File menu.
const LIBRARY_ACTIONS: [(&str, &str, Option<&str>); 7] = [
    ("get-info", "Get Info", Some("CmdOrCtrl+I")),
    ("show-in-finder", "Show in Finder", Some("Shift+CmdOrCtrl+R")),
    ("show-duplicates", "Display Duplicates", None),
    ("show-exact-duplicates", "Display Exact Duplicates", None),
    ("organize-library", "Organize Library…", None),
    ("set-up-player", "Set Up Music Player…", None),
    ("connect-to-server", "Connect to Server…", Some("CmdOrCtrl+K")),
];
const CONTROL_ACTIONS: [(&str, &str, Option<&str>); 7] = [
    ("play-pause", "Play/Pause", None),
    ("next", "Next", Some("CmdOrCtrl+Right")),
    ("previous", "Previous", Some("CmdOrCtrl+Left")),
    ("next-chapter", "Next Chapter", Some("Shift+CmdOrCtrl+Right")),
    ("previous-chapter", "Previous Chapter", Some("Shift+CmdOrCtrl+Left")),
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
            &MenuItem::with_id(app, PREFERENCES.0, PREFERENCES.1, true, PREFERENCES.2)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    // Without update keys there is nothing to check, so the item is left out rather than disabled.
    if updates::configured(app) {
        app_menu.insert(&MenuItem::with_id(app, CHECK_FOR_UPDATES, "Check for Updates…", true, None::<&str>)?, 1)?;
    }
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
    for (id, label, accelerator) in LIBRARY_ACTIONS {
        file.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
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
        if index == 5 {
            controls.append(&PredefinedMenuItem::separator(app)?)?;
        }
        controls.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, EQUALIZER.0, EQUALIZER.1, true, EQUALIZER.2)?,
        ],
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
    } else if id == CHECK_FOR_UPDATES {
        updates::run_check(app, true);
    } else if id == IMPORT_MUSIC {
        import::run(app, None);
    } else if id == IMPORT_XML {
        let handle = app.clone();
        app.dialog().file().set_title("Import Library XML").add_filter("Library XML", &["xml"]).pick_file(move |file| {
            if let Some(path) = file.and_then(|f| f.into_path().ok()) {
                import::run(&handle, Some(path));
            }
        });
    } else if id == PREFERENCES.0
        || id == EQUALIZER.0
        || FILE_ACTIONS.iter().chain(&LIBRARY_ACTIONS).chain(&CONTROL_ACTIONS).any(|(action, _, _)| *action == id)
    {
        let _ = app.emit(MENU_ACTION_EVENT, id);
    }
}
