mod artwork;
mod collection;
mod devices;
mod error;
mod import;
mod library;
mod menu;
mod party;
mod playback;
mod queue;

use antistreaming_playback::Player;
use tauri::Manager;

use collection::CollectionState;
use devices::DevicesState;
use library::LibraryState;
use playback::PlaybackState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .menu(menu::build)
        .on_menu_event(menu::on_event)
        .register_asynchronous_uri_scheme_protocol("artwork", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            // Embedded art can take a moment to pull out of big files; keep it off the IPC thread.
            std::thread::spawn(move || responder.respond(artwork::response(&app, &path)));
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            app.manage(LibraryState::open(data_dir.join("Library.sqlite"))?);
            app.manage(CollectionState::load(data_dir.join("AppleMusicSnapshot.json")));
            app.manage(DevicesState::default());
            let events = playback::spawn_event_worker(app.handle().clone())?;
            let player = Player::new(move |event| {
                let _ = events.send(event);
            })?;
            app.manage(PlaybackState::new(player));
            devices::watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            library::library_tracks,
            library::library_track,
            library::library_stats,
            library::library_set_rating,
            library::library_albums,
            library::library_artists,
            library::library_album_tracks,
            library::library_genres,
            library::library_composers,
            library::library_set_enabled,
            library::library_playlists,
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
            collection::collection_report,
            collection::collection_load_music_app,
            collection::collection_load_xml,
            collection::collection_import_owned,
            collection::collection_locate_folder,
            collection::collection_download_from_icloud,
            playback::player_play_list,
            playback::player_toggle_pause,
            playback::player_next,
            playback::player_previous,
            playback::player_seek,
            playback::player_set_volume,
            playback::player_set_shuffle,
            playback::player_set_repeat,
            playback::player_status,
            party::party_view,
            party::party_play,
            party::party_set_settings,
            party::party_add,
            party::party_remove,
            party::party_move,
            party::party_reshuffle,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AntiStreaming");
}
