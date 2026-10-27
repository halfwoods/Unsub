//! The first-run welcome's backend: whether it is due, that it was answered, and which music folders it can offer.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{CommandError, CommandResult};
use crate::library::LibraryState;

/// Where iCloud Drive keeps a Music folder, relative to the home folder.
const ICLOUD_MUSIC: &str = "Library/Mobile Documents/com~apple~CloudDocs/Music";

/// A folder the welcome offers as a button, named for what the button says.
#[derive(Serialize)]
pub struct WelcomeFolder {
    path: PathBuf,
    label: &'static str,
}

/// Whether the welcome should open now; the rule is the library's.
#[tauri::command]
pub fn welcome_due(state: State<'_, LibraryState>) -> CommandResult<bool> {
    Ok(state.queries().welcome_due()?)
}

#[tauri::command]
pub fn welcome_done(state: State<'_, LibraryState>) -> CommandResult<()> {
    Ok(state.queries().set_welcomed()?)
}

/// The folders worth offering as one-click choices: the Music folder and iCloud Drive's. Async so a slow
/// iCloud Drive can't hold up the window.
#[tauri::command]
pub async fn welcome_folders(app: AppHandle, state: State<'_, LibraryState>) -> CommandResult<Vec<WelcomeFolder>> {
    let paths = app.path();
    let home = paths.home_dir().map_err(|e| CommandError::from(e.to_string()))?;
    let music = crate::user_paths::music(&app).map_err(|e| CommandError::from(e.to_string()))?;
    let media_folder = state.queries().media_settings()?.folder;
    Ok(offered_folders(music, home.join(ICLOUD_MUSIC), media_folder.as_deref()))
}

/// Music is judged by what it holds. iCloud Drive's is judged only by whether it is there: on macOS, listing anything
/// under `Mobile Documents` can raise the iCloud Drive privacy prompt before the user has chosen anything, so that
/// folder is first listed by the import the user starts with it.
fn offered_folders(music: PathBuf, icloud: PathBuf, media_folder: Option<&Path>) -> Vec<WelcomeFolder> {
    let mut folders = Vec::new();
    if holds_music(&music, media_folder) {
        folders.push(WelcomeFolder { path: music, label: "“Music” Folder" });
    }
    if icloud.is_dir() {
        folders.push(WelcomeFolder { path: icloud, label: "iCloud Drive › Music" });
    }
    folders
}

/// Whether `folder` exists and holds something besides hidden files (`.DS_Store`, `.localized`) and the library's own
/// media folder, which Unsub creates inside Music before anything has been added.
///
/// This only lists the folder's own names, stopping at the first that counts. It never goes deeper and never opens a file.
fn holds_music(folder: &Path, media_folder: Option<&Path>) -> bool {
    // The media folder is stored canonical, so entries are compared under the canonical folder.
    let Ok(folder) = fs::canonicalize(folder) else { return false };
    let Ok(entries) = fs::read_dir(&folder) else { return false };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        !name.to_string_lossy().starts_with('.') && media_folder != Some(folder.join(name).as_path())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_with(entries: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for entry in entries {
            fs::create_dir_all(dir.path().join(entry)).unwrap();
        }
        dir
    }

    #[test]
    fn a_folder_with_something_in_it_is_offered() {
        let dir = folder_with(&["Albums"]);
        assert!(holds_music(dir.path(), None));
    }

    #[test]
    fn a_missing_empty_or_hidden_only_folder_is_not() {
        assert!(!holds_music(Path::new("/no/such/folder"), None));
        assert!(!holds_music(folder_with(&[]).path(), None));
        assert!(!holds_music(folder_with(&[".DS_Store", ".localized"]).path(), None));
    }

    #[test]
    fn a_file_is_not_a_folder() {
        let dir = folder_with(&[]);
        let file = dir.path().join("song.mp3");
        fs::write(&file, b"").unwrap();
        assert!(!holds_music(&file, None));
    }

    #[test]
    fn the_media_folder_alone_does_not_count_but_company_does() {
        let dir = folder_with(&["Unsub"]);
        let media = fs::canonicalize(dir.path()).unwrap().join("Unsub");
        assert!(!holds_music(dir.path(), Some(&media)));

        fs::create_dir(dir.path().join("iTunes")).unwrap();
        assert!(holds_music(dir.path(), Some(&media)));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_reached_through_a_link_is_still_compared_by_its_real_path() {
        let dir = folder_with(&["real/Unsub"]);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path().join("real"), &link).unwrap();
        let media = fs::canonicalize(dir.path()).unwrap().join("real/Unsub");
        assert!(!holds_music(&link, Some(&media)));
    }

    fn labels(folders: &[WelcomeFolder]) -> Vec<&str> {
        folders.iter().map(|folder| folder.label).collect()
    }

    #[test]
    fn music_is_offered_for_what_it_holds_and_icloud_for_being_there() {
        let music = folder_with(&["Albums"]);
        let icloud = folder_with(&[]);
        let offered = offered_folders(music.path().to_path_buf(), icloud.path().to_path_buf(), None);
        assert_eq!(labels(&offered), ["“Music” Folder", "iCloud Drive › Music"]);

        let offered = offered_folders(folder_with(&[]).path().to_path_buf(), PathBuf::from("/no/such/folder"), None);
        assert!(offered.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_icloud_folder_is_never_listed() {
        use std::os::unix::fs::PermissionsExt;

        let icloud = folder_with(&[]);
        // A folder that can't be listed is still offered, so offering it can't have listed it.
        fs::set_permissions(icloud.path(), fs::Permissions::from_mode(0o000)).unwrap();
        let offered = offered_folders(PathBuf::from("/no/such/folder"), icloud.path().to_path_buf(), None);
        fs::set_permissions(icloud.path(), fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(labels(&offered), ["iCloud Drive › Music"]);
    }
}
