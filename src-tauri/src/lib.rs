mod artwork;
mod collection;
mod devices;
mod error;
mod import;
mod library;
mod menu;
mod party;
mod playback;
mod podcasts;
mod purchases;
mod queue;
mod sharing;

use antistreaming_playback::Player;
use tauri::Manager;

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
            let artwork_cache = app.path().app_cache_dir()?.join("Artwork");
            app.manage(LibraryState::open(data_dir.join("Library.sqlite"), artwork_cache.clone())?);
            app.manage(CollectionState::load(data_dir.join("AppleMusicSnapshot.json")));
            let library_path = app.state::<LibraryState>().db_path().to_path_buf();
            app.manage(SharingState::load(data_dir.join("Sharing.json"), library_path, artwork_cache));
            app.manage(DevicesState::default());
            let episodes_dir = app.path().audio_dir()?.join("AntiStreaming").join("Podcasts");
            app.manage(PodcastsState::open(&data_dir, episodes_dir)?);
            let purchases_dir = app.path().audio_dir()?.join("AntiStreaming").join("Purchases");
            app.manage(PurchasesState::open(&data_dir, purchases_dir, app.path().download_dir()?)?);
            app.state::<PurchasesState>().apply(app.handle());
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
            playback::player_set_bit_perfect,
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
            sharing::sharing_status,
            sharing::sharing_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running AntiStreaming");
}
