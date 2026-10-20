use std::collections::HashMap;
use std::path::{Path, PathBuf};

use antistreaming_applemusic::{MusicLibrary, PlaylistKind as MusicPlaylistKind};
use antistreaming_library::{ExternalStats, Library, PlaylistKind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::collection::external;
use crate::library::{self, LibraryState};

/// Which of our playlists came from which Music playlist, so importing again updates them in place.
#[derive(Default, Serialize, Deserialize)]
struct PlaylistMapping(HashMap<String, i64>);

impl PlaylistMapping {
    fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_vec(self)?)
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    songs: usize,
    matched: usize,
    playlists_created: usize,
    playlists_updated: usize,
    /// Playlist entries whose song isn't in AntiStreaming (yet); they're left out, not faked.
    entries_skipped: usize,
}

/// Music stores half stars (10, 30, …); AntiStreaming's ratings are whole stars like iTunes 10.
fn whole_stars(rating: u8) -> u8 {
    ((u16::from(rating) + 10) / 20 * 20).min(100) as u8
}

fn import(library: &mut Library, music: &MusicLibrary, mapping: &mut PlaylistMapping) -> Result<ImportSummary, Box<dyn std::error::Error>> {
    let externals: Vec<_> = music.items.iter().map(external).collect();
    let matches = library.match_external(&externals)?;
    let mut ours: HashMap<&str, i64> = HashMap::new();
    let mut stats = Vec::new();
    for (item, found) in music.items.iter().zip(&matches) {
        if let Some(found) = found {
            ours.insert(item.persistent_id.as_str(), found.track_id);
            stats.push((
                found.track_id,
                ExternalStats {
                    play_count: item.play_count,
                    skip_count: item.skip_count,
                    last_played: item.last_played,
                    last_skipped: item.last_skipped,
                    rating: whole_stars(item.rating),
                    date_added: item.date_added,
                    enabled: item.enabled,
                },
            ));
        }
    }
    library.apply_external_stats(&stats)?;

    let mut summary = ImportSummary { songs: music.items.len(), matched: stats.len(), ..ImportSummary::default() };
    let existing: Vec<i64> = library.playlists()?.into_iter().map(|p| p.id).collect();
    // Parents come before children in the export, so a folder's id is known before its contents.
    for playlist in &music.playlists {
        let folder = matches!(playlist.kind, MusicPlaylistKind::Folder);
        let kind = if folder { PlaylistKind::Folder } else { PlaylistKind::Regular };
        let parent = playlist.parent_persistent_id.as_ref().and_then(|p| mapping.0.get(p)).copied();
        let id = match mapping.0.get(&playlist.persistent_id).filter(|id| existing.contains(id)) {
            Some(&id) => {
                library.rename_playlist(id, &playlist.name)?;
                summary.playlists_updated += 1;
                id
            }
            None => {
                let created = library.create_playlist(&playlist.name, kind, parent)?;
                mapping.0.insert(playlist.persistent_id.clone(), created.id);
                summary.playlists_created += 1;
                created.id
            }
        };
        if folder {
            continue;
        }
        // Smart playlists arrive as their current members; Music's rule format isn't portable.
        let members: Vec<i64> = playlist.item_ids.iter().filter_map(|pid| ours.get(pid.as_str()).copied()).collect();
        summary.entries_skipped += playlist.item_ids.len() - members.len();
        // Every member counts here, not just music, so the whole old list is cleared.
        let query = antistreaming_library::TrackQuery { kind: None, playlist: Some(id), ..Default::default() };
        let current = library.tracks(&query)?.len() as u32;
        library.remove_from_playlist(id, &(0..current).collect::<Vec<_>>())?;
        library.add_to_playlist(id, &members, None)?;
    }
    Ok(summary)
}

fn report(app: &AppHandle, result: Result<ImportSummary, String>) {
    let (title, text, kind) = match result {
        Ok(s) => (
            "Import Complete",
            format!(
                "Matched {} of {} songs and merged their play counts, ratings and dates.\n{} playlists created, {} updated.{}",
                s.matched,
                s.songs,
                s.playlists_created,
                s.playlists_updated,
                if s.entries_skipped > 0 {
                    format!(
                        "\n{} playlist entries were left out because those songs aren't in AntiStreaming yet; add them and import again.",
                        s.entries_skipped
                    )
                } else {
                    String::new()
                }
            ),
            MessageDialogKind::Info,
        ),
        Err(message) => ("Import Failed", message, MessageDialogKind::Error),
    };
    app.dialog().message(text).title(title).kind(kind).show(|_| {});
    let _ = app.emit(library::CHANGED_EVENT, ());
}

/// File › Import Music Library: merges stats and playlists from the Music app (or an exported XML).
pub fn run(app: &AppHandle, xml: Option<PathBuf>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<ImportSummary, Box<dyn std::error::Error>> {
            let music = match xml {
                Some(path) => antistreaming_applemusic::read_xml_library(&path)?,
                #[cfg(target_os = "macos")]
                None => antistreaming_applemusic::read_system_music_library()?,
                #[cfg(not(target_os = "macos"))]
                None => return Err("Reading the Music app directly needs macOS; choose an exported Library XML.".into()),
            };
            let mapping_path = app.path().app_data_dir()?.join("ImportedPlaylists.json");
            let mut mapping = PlaylistMapping::load(&mapping_path);
            let state = app.state::<LibraryState>();
            let summary = import(&mut state.queries(), &music, &mut mapping)?;
            mapping.save(&mapping_path)?;
            Ok(summary)
        })();
        report(&app, result.map_err(|e| e.to_string()));
    });
}

#[cfg(test)]
mod tests {
    use super::whole_stars;

    #[test]
    fn half_stars_round_to_whole_stars() {
        assert_eq!(whole_stars(0), 0);
        assert_eq!(whole_stars(10), 20);
        assert_eq!(whole_stars(30), 40);
        assert_eq!(whole_stars(80), 80);
        assert_eq!(whole_stars(100), 100);
    }
}
