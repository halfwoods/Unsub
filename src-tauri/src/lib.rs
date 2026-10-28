mod artwork;
mod cast;
mod cd;
mod collection;
mod details;
mod devices;
mod elsewhere;
mod error;
mod import;
mod jobs;
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
mod rescue;
mod sharing;
mod smoke;
mod sync;
#[cfg(test)]
mod test_support;
mod updates;
mod user_paths;
mod video_add;
mod video_export;
mod visualizer;
mod welcome;
mod window;

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
    let smoke_config = smoke::config().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    #[cfg(target_os = "macos")]
    window::keep_full_screen_item_ours();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .menu(menu::build)
        .on_menu_event(menu::on_event)
        .on_window_event(window::on_event)
        .register_asynchronous_uri_scheme_protocol("artwork", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            let query = request.uri().query().map(str::to_owned);
            // Embedded art can take a moment to pull out of big files; keep it off the IPC thread.
            std::thread::spawn(move || responder.respond(artwork::response(&app, &path, query.as_deref())));
        })
        .setup(move |app| {
            let smoke = smoke_config.is_some();
            if smoke {
                app.manage(smoke::Ready::default());
            }
            let data_dir = app.path().app_data_dir()?;
            let music_dir = user_paths::music(app.handle())?;
            let downloads_dir = user_paths::downloads(app.handle())?;
            if let Some(config) = &smoke_config {
                for path in [&data_dir, &app.path().app_cache_dir()?, &music_dir, &downloads_dir] {
                    config.check_path(path)?;
                }
            }
            if !smoke {
                migrate_data_dir(&data_dir.with_file_name(OLD_IDENTIFIER), &data_dir);
            }
            let artwork_cache = app.path().app_cache_dir()?.join("Artwork");
            app.manage(LibraryState::open(data_dir.join("Library.sqlite"), artwork_cache.clone())?);
            app.manage(CollectionState::load(data_dir.join("AppleMusicSnapshot.json"), data_dir.join("StreamedSongs.json")));
            app.state::<LibraryState>().media_folder(&music_dir.join("Unsub"))?;
            let library_path = app.state::<LibraryState>().db_path().to_path_buf();
            app.manage(SharingState::load(data_dir.join("Sharing.json"), library_path, artwork_cache));
            app.manage(remote::RemoteState::load(data_dir.join("Remote.json"), app.path().app_cache_dir()?.join("Shared Songs")));
            app.manage(DevicesState::default());
            app.manage(video_add::PreviewState::default());
            app.manage(jobs::Jobs::default());
            app.manage(details::DetailsState::default());
            app.manage(import::ImportState::default());
            app.manage(elsewhere::ElsewhereState::default());
            app.manage(updates::UpdatesState::load(data_dir.join("Updates.json")));
            app.manage(loudness::LoudnessState::default());
            app.manage(visualizer::VisualizerState::default());
            app.manage(sync::SyncState::load(data_dir.join("DeviceSync.json")));
            if !smoke {
                sync::tidy(app.handle());
            }
            let episodes_dir = music_dir.join("Unsub").join("Podcasts");
            app.manage(PodcastsState::open(&data_dir, episodes_dir)?);
            let purchases_dir = music_dir.join("Unsub").join("Purchases");
            app.manage(PurchasesState::open(&data_dir, purchases_dir, downloads_dir)?);
            if !smoke {
                PurchasesState::apply(app.handle());
            }
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
            if !smoke {
                cast::browse(app.handle());
                devices::watch(app.handle().clone());
                library::check_missing(app.handle());
                details::resume_on_launch(app.handle());
                remote::browse(app.handle());
                cd::watch(app.handle().clone());
                updates::watch(app.handle().clone());
            }
            if let Some(config) = smoke_config.clone() {
                smoke::start(app.handle().clone(), config);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            smoke::smoke_frontend_ready,
            library::library_tracks,
            library::library_track,
            library::library_lyrics,
            library::library_chapters,
            library::library_stats,
            library::library_set_rating,
            library::library_albums,
            library::library_artists,
            library::library_artist_albums,
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
            library::library_duplicate_playlist,
            library::library_export_playlist,
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
            collection::collection_remove_streamed,
            collection::collection_forget_streamed,
            import::import_music_inspect,
            import::import_music_run,
            import::import_music_cancel,
            elsewhere::elsewhere_inspect,
            elsewhere::elsewhere_run,
            elsewhere::elsewhere_cancel,
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
            podcasts::podcasts_directory_categories,
            podcasts::podcasts_directory_search,
            podcasts::podcasts_directory_top,
            podcasts::podcasts_app_shows,
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
            jobs::job_cancel,
            details::details_status,
            details::details_start,
            details::details_set_paused,
            rescue::device_rescue_preview,
            rescue::device_rescue,
            video_export::device_videos,
            video_export::device_video_export_preview,
            video_export::device_video_export,
            video_add::device_video_add_preview,
            video_add::device_video_add_cancel_preview,
            video_add::device_video_add,
            updates::updates_status,
            updates::updates_set_auto_check,
            updates::updates_dismiss,
            updates::updates_install,
            sharing::sharing_status,
            sharing::sharing_update,
            sharing::sharing_generate_password,
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
            welcome::welcome_due,
            welcome::welcome_done,
            welcome::welcome_folders,
            window::window_double_click_action,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Unsub")
        .run(|app, event| match event {
            // A speaker left playing would carry on from a server that is gone, then go quiet.
            tauri::RunEvent::Exit => {
                if let Some(cast) = app.try_state::<cast::CastState>() {
                    cast.end();
                }
            }
            // Closing the window only hid it, so the Dock icon is how it comes back.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { has_visible_windows: false, .. } => window::show(app),
            _ => {}
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

/// Saves a file that holds passwords, readable by this user only. It is built under another name and renamed into place, so
/// it is never readable by others for a moment, and a file an older version left readable is replaced, not kept.
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let temp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&temp).and_then(|mut file| file.write_all(contents)).and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
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
    fn a_legacy_library_with_committed_wal_data_reopens_with_its_songs_and_history() {
        use unsub_library::{Library, PlaylistKind, TrackQuery};

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("snapshot source");
        let old = root.path().join(OLD_IDENTIFIER);
        let new = root.path().join("new");
        let database = source.join("Library.sqlite");
        let music = root.path().join("music");
        std::fs::create_dir_all(&music).unwrap();
        let original = music.join("saved.mp3");
        let audio = include_bytes!("../../crates/library/tests/fixtures/halo.mp3");
        std::fs::write(&original, audio).unwrap();
        // Checkpoint the schema first; the later song and its history will live in the WAL.
        drop(Library::open(&database).unwrap());
        let mut library = Library::open(&database).unwrap();
        library.add_paths(std::slice::from_ref(&music), |_| {}).unwrap();
        let id = library.tracks(&TrackQuery::default()).unwrap()[0].id;
        library.set_rating(id, 100).unwrap();
        library.record_play(id, 1_700_000_000).unwrap();
        let playlist = library.create_playlist("Legacy favorites", PlaylistKind::Regular, None).unwrap();
        library.add_to_playlist(playlist.id, &[id], None).unwrap();
        let tracks = library.tracks(&TrackQuery::default()).unwrap();
        let playlists = library.playlists().unwrap();

        // Snapshot a quiescent open connection to model a previous process leaving a committed WAL.
        // Closing that connection before copying would checkpoint it and make this a DB-only test.
        assert!(std::fs::metadata(source.join("Library.sqlite-wal")).unwrap().len() > 32);
        std::fs::create_dir(&old).unwrap();
        for name in FILES {
            std::fs::copy(source.join(name), old.join(name)).unwrap();
        }
        let without_wal = root.path().join("without-wal.sqlite");
        std::fs::copy(&database, &without_wal).unwrap();
        assert_eq!(Library::open(&without_wal).unwrap().stats(&TrackQuery::default()).unwrap().count, 0);
        drop(library);

        migrate_data_dir(&old, &new);

        assert!(!old.exists());
        let state = LibraryState::open(new.join("Library.sqlite"), root.path().join("Artwork")).unwrap();
        state.media_folder(&root.path().join("Music/Unsub")).unwrap();
        let mut restored = state.queries();
        assert_eq!(restored.tracks(&TrackQuery::default()).unwrap(), tracks);
        assert_eq!(restored.playlists().unwrap(), playlists);
        assert_eq!(restored.tracks(&TrackQuery { playlist: Some(playlist.id), ..TrackQuery::default() }).unwrap(), tracks);
        assert!(!restored.welcome_due().unwrap());
        assert_eq!(std::fs::read(original).unwrap(), audio);
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

    #[cfg(unix)]
    #[test]
    fn private_files_are_replaced_with_ones_only_their_owner_can_open() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Sharing.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1, "no temporary file is left");
    }

    #[test]
    fn a_failed_private_write_leaves_the_old_file_as_it_was() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Remote.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::create_dir(path.with_extension("tmp")).unwrap();

        assert!(write_private(&path, b"new").is_err());

        assert_eq!(std::fs::read(&path).unwrap(), b"old");
    }
}
