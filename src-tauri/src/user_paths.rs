//! Respect system-configured (including localized) folders, with conventional defaults
//! for minimal Linux profiles that have no XDG user-dirs configuration.
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub fn music(app: &AppHandle) -> tauri::Result<PathBuf> {
    resolve(app.path().audio_dir(), || app.path().home_dir(), "Music")
}

pub fn downloads(app: &AppHandle) -> tauri::Result<PathBuf> {
    resolve(app.path().download_dir(), || app.path().home_dir(), "Downloads")
}

/// Resolves a path only. Callers retain ownership of whether/when to create it.
fn resolve<E>(preferred: Result<PathBuf, E>, home: impl FnOnce() -> Result<PathBuf, E>, folder: &str) -> Result<PathBuf, E> {
    preferred.or_else(|_| home().map(|home| home.join(folder)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_and_localized_paths_are_preserved_without_resolving_home() {
        for preferred in ["/media/Musik", "/home/person/Téléchargements"] {
            let resolved = resolve::<&str>(Ok(preferred.into()), || panic!("home must not be consulted"), "Music").unwrap();
            assert_eq!(resolved, PathBuf::from(preferred));
        }
    }

    #[test]
    fn missing_system_folders_fall_back_without_creating_directories() {
        let root = tempfile::tempdir().unwrap();
        for folder in ["Music", "Downloads"] {
            let resolved = resolve(Err("unavailable"), || Ok(root.path().to_path_buf()), folder).unwrap();
            assert_eq!(resolved, root.path().join(folder));
            assert!(!resolved.exists());
        }
    }

    #[test]
    fn missing_home_is_reported_instead_of_inventing_a_relative_path() {
        assert_eq!(resolve(Err("system folder unavailable"), || Err("home unavailable"), "Music"), Err("home unavailable"));
    }
}
