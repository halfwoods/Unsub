mod artwork;
mod cast;
mod cd;
mod collection;
mod devices;
mod error;
mod import;
mod library;
mod loudness;
mod menu;
mod party;
mod playback;
mod podcasts;
mod purchases;
mod queue;
mod radio;
mod remote;
mod sharing;
mod sync;
#[cfg(test)]
mod test_support;
mod updates;
mod visualizer;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;
use unsub_playback::Player;

use collection::CollectionState;
use devices::DevicesState;
use library::LibraryState;
use playback::PlaybackState;
use podcasts::PodcastsState;
use purchases::PurchasesState;
use sharing::SharingState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .menu(menu::build)
        .on_menu_event(menu::on_event)
        .register_asynchronous_uri_scheme_protocol("artwork", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            let query = request.uri().query().map(str::to_owned);
            // Embedded art can take a moment to pull out of big files; keep it off the IPC thread.
            std::thread::spawn(move || responder.respond(artwork::response(&app, &path, query.as_deref())));
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            migrate_data_dir(&data_dir.with_file_name(OLD_IDENTIFIER), &data_dir);
            let artwork_cache = app.path().app_cache_dir()?.join("Artwork");
            app.manage(LibraryState::open(data_dir.join("Library.sqlite"), artwork_cache.clone())?);
            app.manage(CollectionState::load(data_dir.join("AppleMusicSnapshot.json")));
            app.state::<LibraryState>().media_folder(&app.path().audio_dir()?.join("Unsub"))?;
            let library_path = app.state::<LibraryState>().db_path().to_path_buf();
            app.manage(SharingState::load(data_dir.join("Sharing.json"), library_path, artwork_cache));
            app.manage(remote::RemoteState::load(data_dir.join("Remote.json"), app.path().app_cache_dir()?.join("Shared Songs")));
            app.manage(DevicesState::default());
            app.manage(import::ImportState::default());
            app.manage(updates::UpdatesState::load(data_dir.join("Updates.json")));
            app.manage(loudness::LoudnessState::default());
            app.manage(visualizer::VisualizerState::default());
            app.manage(sync::SyncState::load(data_dir.join("DeviceSync.json")));
            sync::tidy(app.handle());
            let episodes_dir = app.path().audio_dir()?.join("Unsub").join("Podcasts");
            app.manage(PodcastsState::open(&data_dir, episodes_dir)?);
            let purchases_dir = app.path().audio_dir()?.join("Unsub").join("Purchases");
            app.manage(PurchasesState::open(&data_dir, purchases_dir, app.path().download_dir()?)?);
            app.state::<PurchasesState>().apply(app.handle());
            let radio_cache = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&radio_cache)?;
            app.manage(radio::RadioState::new(radio_cache.join("RadioGenres.json")));
            let events = playback::spawn_event_worker(app.handle().clone())?;
            // While a speaker plays, the engine is stopped and what it says about stopping is not news.
            let casting = Arc::new(AtomicBool::new(false));
            let engine_events = events.clone();
            let engine_silenced = Arc::clone(&casting);
            let player = Player::new(move |event| {
                if !engine_silenced.load(Ordering::Relaxed) {
                    let _ = engine_events.send(event);
                }
            })?;
            app.manage(cast::CastState::new(events, casting, app.path().app_cache_dir()?.join("Cast")));
            app.manage(PlaybackState::new(player, app.handle().clone()));
            cast::browse(app.handle());
            devices::watch(app.handle().clone());
            library::check_missing(app.handle());
            remote::browse(app.handle());
            cd::watch(app.handle().clone());
            updates::check_on_launch(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            library::library_tracks,
            library::library_track,
            library::library_lyrics,
            library::library_chapters,
            library::library_stats,
            library::library_set_rating,
            library::library_albums,
            library::library_artists,
            library::library_album_tracks,
            library::library_genres,
            library::library_composers,
            library::library_set_enabled,
            library::library_playlists,
            library::library_mixes,
            library::library_mix_for,
            library::library_create_playlist,
            library::library_rename_playlist,
            library::library_delete_playlist,
            library::library_move_playlist,
            library::library_add_to_playlist,
            library::library_remove_from_playlist,
            library::library_move_in_playlist,
            library::library_set_smart_rules,
            library::library_add_paths,
            devices::devices_list,
            devices::device_tracks,
            devices::device_stats,
            devices::device_eject,
            devices::players_list,
            devices::player_set_up,
            devices::player_eject,
            remote::remote_libraries,
            remote::remote_connect,
            remote::remote_open,
            remote::remote_forget,
            remote::remote_tracks,
            collection::collection_report,
            collection::collection_load_music_app,
            collection::collection_load_xml,
            collection::collection_import_owned,
            collection::collection_locate_folder,
            collection::collection_download_from_icloud,
            import::import_music_inspect,
            import::import_music_run,
            import::import_music_cancel,
            playback::player_play_list,
            playback::player_toggle_pause,
            playback::player_next,
            playback::player_previous,
            playback::player_seek,
            playback::player_set_volume,
            playback::player_set_shuffle,
            playback::player_set_repeat,
            playback::player_status,
            playback::player_set_bit_perfect,
            playback::player_set_sound_check,
            playback::player_set_equalizer,
            playback::player_equalizer_presets,
            playback::player_set_crossfade,
            party::party_view,
            party::party_play,
            party::party_set_settings,
            party::party_add,
            party::party_remove,
            party::party_move,
            party::party_reshuffle,
            podcasts::podcasts_feeds,
            podcasts::podcasts_episodes,
            podcasts::podcasts_subscribe,
            podcasts::podcasts_unsubscribe,
            podcasts::podcasts_refresh,
            podcasts::podcasts_set_settings,
            podcasts::podcasts_set_played,
            podcasts::podcasts_download,
            purchases::purchases_status,
            purchases::purchases_set_watching,
            purchases::purchases_scan,
            purchases::purchases_accept,
            purchases::purchases_ignore,
            purchases::purchases_links,
            library::library_edit_tracks,
            library::library_clean_up_preview,
            library::library_clean_up,
            library::library_cancel_clean_up,
            library::library_media_settings,
            library::library_set_media_settings,
            library::library_organize,
            library::library_duplicates,
            library::library_relink,
            library::library_delete,
            cd::cd_list,
            cd::cd_tracks,
            cd::cd_set_checked,
            cd::cd_choose_release,
            cd::cd_import,
            cd::cd_stop_import,
            cd::cd_eject,
            sync::device_sync_settings,
            sync::device_set_sync_settings,
            sync::device_sync_preview,
            sync::device_sync,
            sync::device_cancel_sync,
            updates::updates_status,
            updates::updates_set_auto_check,
            updates::updates_install,
            sharing::sharing_status,
            sharing::sharing_update,
            radio::radio_genres,
            radio::radio_stations,
            radio::radio_search,
            radio::radio_my_stations,
            radio::radio_recent,
            radio::radio_add,
            radio::radio_remove,
            radio::radio_play,
            radio::radio_play_directory,
            radio::radio_open_stream,
            radio::radio_import,
            visualizer::visualizer_start,
            visualizer::visualizer_stop,
            visualizer::visualizer_mic_devices,
            cast::cast_speakers,
            cast::cast_select,
            cast::cast_open_airplay,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Unsub")
        .run(|app, event| {
            // A speaker left playing would carry on from a server that is gone, then go quiet.
            if matches!(event, tauri::RunEvent::Exit)
                && let Some(cast) = app.try_state::<cast::CastState>()
            {
                cast.end();
            }
        });
}

/// The bundle identifier from before the rename. The data folder is named after the identifier and
/// lives beside the new one in every OS's app data root (Application Support, Roaming AppData,
/// `$XDG_DATA_HOME`), so the old folder is the new one with its last component swapped.
const OLD_IDENTIFIER: &str = "org.antistreaming.app";

/// Brings the library along from the data folder the app had under its old identifier. The folder
/// is renamed whole, which is atomic on one volume, so there is never a half-moved library; if the
/// rename fails the old folder stays as it was and the app starts with a fresh one.
fn migrate_data_dir(old: &Path, new: &Path) {
    if new.exists() || !old.is_dir() {
        return;
    }
    if let Err(error) = std::fs::rename(old, new) {
        eprintln!("[migrate] couldn't move {} to {}: {error}", old.display(), new.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: [&str; 3] = ["Library.sqlite", "Library.sqlite-shm", "Library.sqlite-wal"];

    fn write_library(dir: &Path, contents: &str) {
        std::fs::create_dir(dir).unwrap();
        for name in FILES {
            std::fs::write(dir.join(name), format!("{contents} {name}")).unwrap();
        }
    }

    fn library_files(contents: &str) -> Vec<String> {
        FILES.iter().map(|name| format!("{contents} {name}")).collect()
    }

    fn read_library(dir: &Path) -> Vec<String> {
        let mut files: Vec<String> =
            std::fs::read_dir(dir).unwrap().map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()).collect();
        files.sort();
        files
    }

    #[test]
    fn the_old_folder_moves_whole_when_only_it_exists() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(OLD_IDENTIFIER), root.path().join("new"));
        write_library(&old, "old");

        migrate_data_dir(&old, &new);

        assert!(!old.exists());
        assert_eq!(read_library(&new), library_files("old"));
    }

    #[test]
    fn both_folders_are_left_alone_when_the_new_one_exists() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(OLD_IDENTIFIER), root.path().join("new"));
        write_library(&old, "old");
        write_library(&new, "new");

        migrate_data_dir(&old, &new);

        assert_eq!(read_library(&old), library_files("old"));
        assert_eq!(read_library(&new), library_files("new"));
    }

    #[test]
    fn nothing_happens_when_neither_folder_exists() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(OLD_IDENTIFIER), root.path().join("new"));

        migrate_data_dir(&old, &new);

        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_failed_move_leaves_the_old_folder_untouched() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(OLD_IDENTIFIER), root.path().join("no such parent").join("new"));
        write_library(&old, "old");

        migrate_data_dir(&old, &new);

        assert!(!new.exists());
        assert_eq!(read_library(&old), library_files("old"));
    }
}
