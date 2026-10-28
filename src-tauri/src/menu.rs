use tauri::menu::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime, Window, Wry};
use tauri_plugin_dialog::DialogExt;

use crate::{library, updates, window};

/// Playlist and Controls items are handled by the UI, which owns the selection and "what Play means".
/// So are the imports, whose sheet asks where the music files are now.
pub const MENU_ACTION_EVENT: &str = "menu://action";
/// A native file/folder picker was accepted; cancellation and unrelated library changes never send this.
pub const MUSIC_ADD_ACCEPTED_EVENT: &str = "menu://music-add-accepted";

const ADD_TO_LIBRARY: &str = "add-to-library";
const ADD_MUSIC_FILES: &str = "add-music-files";
const IMPORT_MUSIC: &str = "import-music-library";
const IMPORT_XML: &str = "import-library-xml";
const IMPORT_ELSEWHERE: &str = "import-elsewhere";
const CHECK_FOR_UPDATES: &str = "check-for-updates";
const DELETE: &str = "delete";
/// The design follows classic iTunes, so the About box makes clear Unsub isn't Apple's.
const NOT_APPLE: &str = "Not affiliated with or endorsed by Apple. iTunes, iPod and Apple Music are trademarks of Apple Inc.";
const PREFERENCES: (&str, &str, Option<&str>) = ("preferences", "Preferences…", Some("CmdOrCtrl+,"));
const EQUALIZER: (&str, &str, Option<&str>) = ("equalizer", "Equalizer", Some("Alt+CmdOrCtrl+E"));
const SHOW_VISUALIZER: (&str, &str, Option<&str>) = ("show-visualizer", "Show Visualizer", Some("CmdOrCtrl+T"));
/// The system's own Close Window asks the window's close button to act, and a borderless window has none.
#[cfg(target_os = "macos")]
const CLOSE_WINDOW: (&str, &str, Option<&str>) = ("close-window", "Close Window", Some("CmdOrCtrl+W"));
#[cfg(target_os = "macos")]
const FULL_SCREEN_KEYS: &str = "Ctrl+Cmd+F";
#[cfg(not(target_os = "macos"))]
const FULL_SCREEN_KEYS: &str = "F11";
const FULL_SCREEN: (&str, &str, Option<&str>) = ("toggle-full-screen", "Enter Full Screen", Some(FULL_SCREEN_KEYS));
const EXIT_FULL_SCREEN: &str = "Exit Full Screen";
/// The system's Minimize and Zoom press the window's own buttons too, so they are ours on macOS.
#[cfg(target_os = "macos")]
const MINIMIZE_WINDOW: (&str, &str, Option<&str>) = ("minimize-window", "Minimize", Some("CmdOrCtrl+M"));
#[cfg(target_os = "macos")]
const ZOOM_WINDOW: (&str, &str, Option<&str>) = ("zoom-window", "Zoom", None);
/// A way back to the window besides the Dock icon, since closing it only hides it.
#[cfg(target_os = "macos")]
const SHOW_WINDOW: (&str, &str, Option<&str>) = ("show-window", "Unsub", None);
const VISUALIZER_STYLES: [(&str, &str, Option<&str>); 8] = [
    ("visualizer-classic", "Classic", None),
    ("visualizer-lights", "Lights", None),
    ("visualizer-spectrum", "Spectrum", None),
    ("visualizer-scope", "Scope", None),
    ("visualizer-album", "Album", None),
    ("visualizer-pipes", "Pipes", None),
    ("visualizer-starfield", "Starfield", None),
    ("visualizer-lines", "Lines", None),
];
const FILE_ACTIONS: [(&str, &str, Option<&str>); 5] = [
    ("new-playlist", "New Playlist", Some("CmdOrCtrl+N")),
    ("new-playlist-from-selection", "New Playlist from Selection", Some("Shift+CmdOrCtrl+N")),
    ("new-smart-playlist", "New Smart Playlist…", Some("Alt+CmdOrCtrl+N")),
    ("new-playlist-folder", "New Playlist Folder", Some("Shift+Alt+CmdOrCtrl+N")),
    ("open-stream", "Open Stream…", Some("CmdOrCtrl+U")),
];
/// The platform's own name for its file manager, which the context menus in src/browser/songMenu.ts use too.
#[cfg(target_os = "macos")]
const SHOW_IN_FILE_MANAGER: &str = "Show in Finder";
#[cfg(windows)]
const SHOW_IN_FILE_MANAGER: &str = "Show in Explorer";
#[cfg(not(any(target_os = "macos", windows)))]
const SHOW_IN_FILE_MANAGER: &str = "Show in File Manager";
/// The selection-dependent half of iTunes 10's File menu.
const LIBRARY_ACTIONS: [(&str, &str, Option<&str>); 7] = [
    ("get-info", "Get Info", Some("CmdOrCtrl+I")),
    ("show-in-finder", SHOW_IN_FILE_MANAGER, Some("Shift+CmdOrCtrl+R")),
    ("show-duplicates", "Display Duplicates", None),
    ("show-exact-duplicates", "Display Exact Duplicates", None),
    ("organize-library", "Organize Library…", None),
    ("set-up-player", "Set Up Music Player…", None),
    ("connect-to-server", "Connect to Server…", Some("CmdOrCtrl+K")),
];
const CLEAN_UP: (&str, &str, Option<&str>) = ("clean-up", "Clean Up Song Info…", None);
const SONG_DETAILS: (&str, &str, Option<&str>) = ("get-song-details", "Get Song Details from iCloud…", None);
const WELCOME: (&str, &str, Option<&str>) = ("welcome", "Welcome to Unsub…", None);
const CONTROL_ACTIONS: [(&str, &str, Option<&str>); 7] = [
    ("play-pause", "Play/Pause", None),
    ("next", "Next", Some("CmdOrCtrl+Right")),
    ("previous", "Previous", Some("CmdOrCtrl+Left")),
    ("next-chapter", "Next Chapter", Some("Shift+CmdOrCtrl+Right")),
    ("previous-chapter", "Previous Chapter", Some("Shift+CmdOrCtrl+Left")),
    ("volume-up", "Increase Volume", Some("CmdOrCtrl+Up")),
    ("volume-down", "Decrease Volume", Some("CmdOrCtrl+Down")),
];

/// Kept so the label can follow the window in and out of full screen, however it got there.
struct FullScreenItem<R: Runtime>(MenuItem<R>);

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let info = app.package_info();
    // macOS shows credits and the other platforms show comments, so the notice goes in both.
    let about = AboutMetadata {
        name: Some(info.name.clone()),
        version: Some(info.version.to_string()),
        credits: Some(NOT_APPLE.to_owned()),
        comments: Some(NOT_APPLE.to_owned()),
        ..AboutMetadata::default()
    };
    let app_menu = Submenu::with_items(
        app,
        "Unsub",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Unsub"), Some(about))?,
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
    file.append(&MenuItem::with_id(app, ADD_TO_LIBRARY, "Add Music Folder…", true, Some("CmdOrCtrl+O"))?)?;
    file.append(&MenuItem::with_id(app, ADD_MUSIC_FILES, "Add Music Files…", true, Some("Shift+CmdOrCtrl+O"))?)?;
    #[cfg(target_os = "macos")]
    file.append(&MenuItem::with_id(app, IMPORT_MUSIC, "Import Music Library", true, None::<&str>)?)?;
    file.append(&MenuItem::with_id(app, IMPORT_XML, "Import Library XML…", true, None::<&str>)?)?;
    file.append(&MenuItem::with_id(app, IMPORT_ELSEWHERE, "Import from Spotify, YouTube Music or ListenBrainz…", true, None::<&str>)?)?;
    file.append(&PredefinedMenuItem::separator(app)?)?;
    for (id, label, accelerator) in LIBRARY_ACTIONS {
        file.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    file.append(&PredefinedMenuItem::separator(app)?)?;
    #[cfg(target_os = "macos")]
    file.append(&MenuItem::with_id(app, CLOSE_WINDOW.0, CLOSE_WINDOW.1, true, CLOSE_WINDOW.2)?)?;
    // Elsewhere closing the window quits, so the system's own item (Alt+F4) stays rather than a Ctrl+W that would quit by accident.
    #[cfg(not(target_os = "macos"))]
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
            // No accelerator: Backspace would be swallowed in every text field.
            &MenuItem::with_id(app, DELETE, "Delete", true, None::<&str>)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let visualizer_styles = Submenu::new(app, "Visualizer", true)?;
    for (id, label, accelerator) in VISUALIZER_STYLES {
        visualizer_styles.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    let full_screen = MenuItem::with_id(app, FULL_SCREEN.0, FULL_SCREEN.1, true, FULL_SCREEN.2)?;
    app.manage(FullScreenItem(full_screen.clone()));
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &MenuItem::with_id(app, "show-view-options", "Show View Options…", true, Some("CmdOrCtrl+J"))?,
            &MenuItem::with_id(app, "toggle-column-browser", "Show/Hide Column Browser", true, Some("CmdOrCtrl+B"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, SHOW_VISUALIZER.0, SHOW_VISUALIZER.1, true, SHOW_VISUALIZER.2)?,
            &visualizer_styles,
            &PredefinedMenuItem::separator(app)?,
            &full_screen,
        ],
    )?;
    let controls = Submenu::new(app, "Controls", true)?;
    for (index, (id, label, accelerator)) in CONTROL_ACTIONS.into_iter().enumerate() {
        if index == 5 {
            controls.append(&PredefinedMenuItem::separator(app)?)?;
        }
        controls.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    let advanced = Submenu::with_items(app, "Advanced", true, &[&MenuItem::with_id(app, CLEAN_UP.0, CLEAN_UP.1, true, CLEAN_UP.2)?])?;
    // Only where there are cloud placeholders to fetch and give back.
    #[cfg(any(target_os = "macos", windows))]
    advanced.append(&MenuItem::with_id(app, SONG_DETAILS.0, SONG_DETAILS.1, true, SONG_DETAILS.2)?)?;
    let window = Submenu::new(app, "Window", true)?;
    #[cfg(target_os = "macos")]
    for (id, label, accelerator) in [MINIMIZE_WINDOW, ZOOM_WINDOW] {
        window.append(&MenuItem::with_id(app, id, label, true, accelerator)?)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        window.append(&PredefinedMenuItem::minimize(app, None)?)?;
        window.append(&PredefinedMenuItem::maximize(app, None)?)?;
    }
    window.append(&PredefinedMenuItem::separator(app)?)?;
    window.append(&MenuItem::with_id(app, EQUALIZER.0, EQUALIZER.1, true, EQUALIZER.2)?)?;
    #[cfg(target_os = "macos")]
    window.insert(&MenuItem::with_id(app, SHOW_WINDOW.0, SHOW_WINDOW.1, true, SHOW_WINDOW.2)?, 3)?;
    let help = Submenu::with_items(app, "Help", true, &[&MenuItem::with_id(app, WELCOME.0, WELCOME.1, true, WELCOME.2)?])?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &view, &controls, &advanced, &window, &help])
}

/// The View menu's item says what it would do, which depends on where the window is now.
pub fn sync_full_screen_label(window: &Window) {
    if let Some(item) = window.try_state::<FullScreenItem<Wry>>() {
        let label = if window.is_fullscreen().unwrap_or(false) { EXIT_FULL_SCREEN } else { FULL_SCREEN.1 };
        let _ = item.0.set_text(label);
    }
}

/// The items that do the window's own work, which only macOS has to do for itself. Says whether `id` was one.
#[cfg(target_os = "macos")]
fn window_item(app: &AppHandle, id: &str) -> bool {
    if id == CLOSE_WINDOW.0 {
        window::close(app);
    } else if id == MINIMIZE_WINDOW.0 {
        window::minimize(app);
    } else if id == ZOOM_WINDOW.0 {
        window::zoom(app);
    } else if id == SHOW_WINDOW.0 {
        window::show(app);
    } else {
        return false;
    }
    true
}

pub fn on_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    #[cfg(target_os = "macos")]
    if window_item(app, id) {
        return;
    }
    if id == ADD_TO_LIBRARY {
        let handle = app.clone();
        app.dialog().file().set_title("Add Music Folder").pick_folders(move |folders| {
            let paths: Vec<_> = folders.unwrap_or_default().into_iter().filter_map(|f| f.into_path().ok()).collect();
            if !paths.is_empty() {
                let _ = handle.emit(MUSIC_ADD_ACCEPTED_EVENT, ());
                library::add_paths(&handle, paths);
            }
        });
    } else if id == ADD_MUSIC_FILES {
        let handle = app.clone();
        app.dialog().file().set_title("Add Music Files").pick_files(move |files| {
            let paths: Vec<_> = files.unwrap_or_default().into_iter().filter_map(|f| f.into_path().ok()).collect();
            if !paths.is_empty() {
                let _ = handle.emit(MUSIC_ADD_ACCEPTED_EVENT, ());
                library::add_paths(&handle, paths);
            }
        });
    } else if CONTROL_ACTIONS.iter().any(|(action, _, _)| *action == id) {
        // Playback keeps working with the window hidden.
        let _ = app.emit(MENU_ACTION_EVENT, id);
    } else if id == CHECK_FOR_UPDATES {
        window::show(app);
        updates::run_check(app, true);
    } else if id == FULL_SCREEN.0 {
        window::show(app);
        window::toggle_full_screen(app);
    } else if id == PREFERENCES.0
        || id == "show-view-options"
        || id == "toggle-column-browser"
        || id == EQUALIZER.0
        || id == IMPORT_MUSIC
        || id == IMPORT_XML
        || id == IMPORT_ELSEWHERE
        || id == DELETE
        || id == SHOW_VISUALIZER.0
        || id == CLEAN_UP.0
        || id == SONG_DETAILS.0
        || id == WELCOME.0
        || FILE_ACTIONS.iter().chain(&LIBRARY_ACTIONS).chain(&VISUALIZER_STYLES).any(|(action, _, _)| *action == id)
    {
        // The UI answers these, and the window it answers in may be hidden.
        window::show(app);
        let _ = app.emit(MENU_ACTION_EVENT, id);
    }
}
