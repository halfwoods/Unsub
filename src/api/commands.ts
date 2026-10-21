import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { VisualizerFrame, VisualizerSource } from "../visualizer/feed";
import type { VisualizerStyle } from "../visualizer/styles";
import type {
  AlbumSummary,
  ArtistSummary,
  AudioCd,
  CdRipStatus,
  Chapter,
  CleanUpDone,
  CleanUpPreview,
  CleanUpProgress,
  CleanUpRules,
  ComposerSummary,
  Device,
  DirectoryStation,
  DoubleClickAction,
  ElsewherePreview,
  ElsewhereService,
  ArtworkChange,
  DetailsProgress,
  DetailsStatus,
  DeviceSyncStatus,
  EditReport,
  DeviceView,
  GenreSummary,
  ImportFormat,
  ImportPreview,
  ImportProgress,
  LibraryStats,
  Lyrics,
  MediaSettings,
  OrganizeProgress,
  Mix,
  OwnershipReport,
  FeedSettings,
  PartySettings,
  PodcastCategory,
  PodcastEpisode,
  PodcastFeed,
  PodcastListing,
  PodcastListings,
  PodcastProgress,
  Purchase,
  RadioGenre,
  RadioLink,
  RadioStation,
  RadioTitle,
  PurchasesStatus,
  StoreLink,
  PartyView,
  PlaybackStatus,
  Player,
  PlaySource,
  PlayerEvent,
  Playlist,
  PlaylistKind,
  RepeatMode,
  RescueOptions,
  RescuePreview,
  RescueScope,
  ScanProgress,
  SharedLibrary,
  SharingSettings,
  Speaker,
  Speakers,
  SyncPreview,
  SyncSettings,
  TrackEdits,
  SharingStatus,
  SmartRules,
  Track,
  TrackQuery,
  InstallEvent,
  MicDevice,
  UpdateResult,
  UpdatesStatus,
  WelcomeFolder,
} from "./types";

export const libraryTracks = (query: TrackQuery) => invoke<Track[]>("library_tracks", { query });
export const libraryAlbums = (query: TrackQuery) => invoke<AlbumSummary[]>("library_albums", { query });
export const libraryArtists = (query: TrackQuery) => invoke<ArtistSummary[]>("library_artists", { query });
export const libraryAlbumTracks = (key: string) => invoke<Track[]>("library_album_tracks", { key });
export const libraryGenres = (query: TrackQuery) => invoke<GenreSummary[]>("library_genres", { query });
export const libraryComposers = (query: TrackQuery) => invoke<ComposerSummary[]>("library_composers", { query });
export const libraryMixes = () => invoke<Mix[]>("library_mixes");
export const libraryMixFor = (trackIds: number[]) => invoke<number[]>("library_mix_for", { trackIds });
export const libraryPlaylists = () => invoke<Playlist[]>("library_playlists");
export const libraryCreatePlaylist = (name: string, kind: PlaylistKind, parentId: number | null, trackIds: number[] = []) =>
  invoke<Playlist>("library_create_playlist", { name, kind, parentId, trackIds });
export const libraryRenamePlaylist = (id: number, name: string) => invoke<void>("library_rename_playlist", { id, name });
export const libraryDeletePlaylist = (id: number) => invoke<void>("library_delete_playlist", { id });
/** Resolves to the copy's id. */
export const libraryDuplicatePlaylist = (id: number) => invoke<number>("library_duplicate_playlist", { id });
export const libraryExportPlaylist = (id: number, path: string) => invoke<void>("library_export_playlist", { id, path });
export const libraryMovePlaylist = (id: number, parentId: number | null, index: number) =>
  invoke<void>("library_move_playlist", { id, parentId, index });
export const libraryAddToPlaylist = (id: number, trackIds: number[], at: number | null = null) =>
  invoke<void>("library_add_to_playlist", { id, trackIds, at });
export const libraryRemoveFromPlaylist = (id: number, positions: number[]) =>
  invoke<void>("library_remove_from_playlist", { id, positions });
export const libraryMoveInPlaylist = (id: number, positions: number[], to: number) =>
  invoke<void>("library_move_in_playlist", { id, positions, to });
export const librarySetSmartRules = (id: number, rules: SmartRules) => invoke<void>("library_set_smart_rules", { id, rules });
export const librarySetEnabled = (trackIds: number[], enabled: boolean) =>
  invoke<void>("library_set_enabled", { trackIds, enabled });
export const libraryTrack = (trackId: number) => invoke<Track | null>("library_track", { trackId });
export const libraryLyrics = (trackId: number) => invoke<Lyrics | null>("library_lyrics", { trackId });
export const libraryChapters = (trackId: number) => invoke<Chapter[]>("library_chapters", { trackId });
export const libraryStats = (query: TrackQuery) => invoke<LibraryStats>("library_stats", { query });
export const libraryAddPaths = (paths: string[]) => invoke<void>("library_add_paths", { paths });
export const librarySetRating = (trackId: number, rating: number) =>
  invoke<void>("library_set_rating", { trackId, rating });

export const devicesList = () => invoke<Device[]>("devices_list");
export const playersList = () => invoke<Player[]>("players_list");

export const remoteLibraries = () => invoke<SharedLibrary[]>("remote_libraries");
/** Logs in to a shared library (`library`), or adds a server by `url`; returns the library's id. */
export const remoteConnect = (library: string | null, url: string | null, name: string | null, username: string, password: string) =>
  invoke<string>("remote_connect", { library, url, name, username, password });
export const remoteOpen = (library: string) => invoke<void>("remote_open", { library });
export const remoteForget = (library: string) => invoke<void>("remote_forget", { library });
export const remoteTracks = (library: string, query: TrackQuery) => invoke<Track[]>("remote_tracks", { library, query });
export const onRemoteChanged = (handler: () => void): Promise<UnlistenFn> => listen("remote://changed", () => handler());
export const playerEject = (deviceId: string) => invoke<void>("player_eject", { deviceId });
export type PlayerPreset = "massStorage" | "android" | "rockbox";
export const playerSetUp = (path: string, name: string, preset: PlayerPreset) => invoke<void>("player_set_up", { path, name, preset });
export const deviceTracks = (deviceId: string, view: DeviceView, query: TrackQuery) =>
  invoke<Track[]>("device_tracks", { deviceId, view, query });
export const deviceStats = (deviceId: string, view: DeviceView, query: TrackQuery) =>
  invoke<LibraryStats>("device_stats", { deviceId, view, query });
export const deviceEject = (deviceId: string) => invoke<void>("device_eject", { deviceId });
export const libraryEditTracks = (trackIds: number[], edits: TrackEdits, artwork: ArtworkChange | null) =>
  invoke<EditReport>("library_edit_tracks", { trackIds, edits, artwork });
export const libraryCleanUpPreview = (trackIds: number[], rules: CleanUpRules) =>
  invoke<CleanUpPreview>("library_clean_up_preview", { trackIds, rules });
/** Starts the clean-up in the background; its progress and end come as events. */
export const libraryCleanUp = (trackIds: number[], rules: CleanUpRules) => invoke<void>("library_clean_up", { trackIds, rules });
export const onCleanUpProgress = (handler: (progress: CleanUpProgress | null) => void): Promise<UnlistenFn> =>
  listen<CleanUpProgress | null>("library://clean-up", (e) => handler(e.payload));
export const onCleanUpDone = (handler: (done: CleanUpDone) => void): Promise<UnlistenFn> =>
  listen<CleanUpDone>("library://clean-up-done", (e) => handler(e.payload));
export const detailsStatus = () => invoke<DetailsStatus>("details_status");
/** Starts reading the songs that are only in iCloud, in the background; its progress comes as events. */
export const detailsStart = () => invoke<void>("details_start");
export const detailsSetPaused = (paused: boolean) => invoke<void>("details_set_paused", { paused });
export const onDetailsProgress = (handler: (progress: DetailsProgress | null) => void): Promise<UnlistenFn> =>
  listen<DetailsProgress | null>("library://details", (e) => handler(e.payload));
export const libraryMediaSettings = () => invoke<MediaSettings>("library_media_settings");
export const librarySetMediaSettings = (folder: string, organize: boolean, copyOnAdd: boolean) =>
  invoke<void>("library_set_media_settings", { folder, organize, copyOnAdd });
export const libraryOrganize = () => invoke<void>("library_organize");
export const libraryDuplicates = (exact: boolean) => invoke<number[][]>("library_duplicates", { exact });
export const libraryRelink = (trackId: number, path: string, searchFolder: boolean) =>
  invoke<number>("library_relink", { trackId, path, searchFolder });
export const libraryDelete = (trackIds: number[], moveFilesToTrash: boolean) =>
  invoke<void>("library_delete", { trackIds, moveFilesToTrash });
export const onOrganizeProgress = (handler: (progress: OrganizeProgress | null) => void): Promise<UnlistenFn> =>
  listen<OrganizeProgress | null>("library://organize", (e) => handler(e.payload));
export const deviceSyncSettings = (deviceId: string) => invoke<SyncSettings>("device_sync_settings", { deviceId });
export const deviceSetSyncSettings = (deviceId: string, settings: SyncSettings) =>
  invoke<void>("device_set_sync_settings", { deviceId, settings });
export const deviceSyncPreview = (deviceId: string) => invoke<SyncPreview>("device_sync_preview", { deviceId });
export const deviceSync = (deviceId: string) => invoke<void>("device_sync", { deviceId });
/** The background work that can be started, which `jobCancel` stops. */
export type JobKind = "import" | "cleanUp" | "sync" | "organize" | "scan" | "podcasts" | "details";
/** Stops the running jobs of a kind at their next look; stopping one that isn't running does nothing. */
export const jobCancel = (kind: JobKind) => invoke<void>("job_cancel", { kind });
/** What Copy to Library would do with these choices; nothing is copied. */
export const deviceRescuePreview = (deviceId: string, scope: RescueScope, options: RescueOptions) =>
  invoke<RescuePreview>("device_rescue_preview", { deviceId, scope, options });
/** Copies in the background, in the place of a sync: its progress and end come as the sync's events. */
export const deviceRescue = (deviceId: string, scope: RescueScope, options: RescueOptions) =>
  invoke<void>("device_rescue", { deviceId, scope, options });

export const cdList = () => invoke<AudioCd[]>("cd_list");
export const cdTracks = (discId: string) => invoke<Track[]>("cd_tracks", { discId });
export const cdSetChecked = (discId: string, numbers: number[], checked: boolean) =>
  invoke<void>("cd_set_checked", { discId, numbers, checked });
export const cdChooseRelease = (discId: string, index: number) => invoke<void>("cd_choose_release", { discId, index });
export const cdImport = (discId: string, format: ImportFormat, verify: boolean) => invoke<void>("cd_import", { discId, format, verify });
export const cdStopImport = () => invoke<void>("cd_stop_import");
export const cdEject = (discId: string) => invoke<void>("cd_eject", { discId });

/** Thumbnail sizes are bucketed so a dragged size slider reuses a few cached images, not hundreds. */
const THUMBNAIL_BUCKETS = [64, 128, 256, 512, 1024];

/**
 * Artwork is served by a custom protocol so <img> can cache it without base64 round trips.
 * Pass `size` (CSS pixels) for a thumbnail sized for this screen.
 */
// CD songs have no embedded art (their cover comes from the Cover Art Archive), and their ids are
// track numbers that would fetch some library song's art, so CDs are excluded by type.
export const artworkUrl = (
  trackId: number,
  source: Exclude<PlaySource, { type: "cd" } | { type: "shared" }> = { type: "library" },
  size?: number,
) => {
  const url = convertFileSrc(source.type === "library" ? String(trackId) : `device/${source.deviceId}/${trackId}`, "artwork");
  if (!size) return url;
  const pixels = size * window.devicePixelRatio;
  return `${url}?size=${THUMBNAIL_BUCKETS.find((b) => b >= pixels) ?? THUMBNAIL_BUCKETS[THUMBNAIL_BUCKETS.length - 1]}`;
};

/** Plays `trackIds` in order starting at `startIndex`; the backend owns the queue so gapless works. */
export const playerPlayList = (source: PlaySource, trackIds: number[], startIndex: number) =>
  invoke<void>("player_play_list", { source, trackIds, startIndex });
export const playerTogglePause = () => invoke<void>("player_toggle_pause");
export const playerNext = () => invoke<void>("player_next");
export const playerPrevious = () => invoke<void>("player_previous");
export const playerSeek = (positionMs: number) => invoke<void>("player_seek", { positionMs });
export const playerSetVolume = (volume: number) => invoke<void>("player_set_volume", { volume });
export const playerSetShuffle = (shuffle: boolean) => invoke<void>("player_set_shuffle", { shuffle });
export const playerSetRepeat = (repeat: RepeatMode) => invoke<void>("player_set_repeat", { repeat });
export const playerStatus = () => invoke<PlaybackStatus>("player_status");
export const playerSetBitPerfect = (enabled: boolean) => invoke<void>("player_set_bit_perfect", { enabled });
export const playerSetSoundCheck = (enabled: boolean) => invoke<void>("player_set_sound_check", { enabled });
export const playerSetEqualizer = (equalizer: { preampDb: number; bands: number[] } | null, preventClipping: boolean) =>
  invoke<void>("player_set_equalizer", { equalizer, preventClipping });
export const playerEqualizerPresets = () => invoke<{ name: string; bands: number[] }[]>("player_equalizer_presets");
export const playerSetCrossfade = (seconds: number | null) => invoke<void>("player_set_crossfade", { seconds });

export const castSpeakers = () => invoke<Speakers>("cast_speakers");
/** Plays on the speaker `id`, or on this computer with null, carrying on from where the song is. */
export const castSelect = (id: string | null) => invoke<void>("cast_select", { id });
export const castOpenAirplay = () => invoke<void>("cast_open_airplay");
export const onCastTarget = (handler: (target: Speaker | null) => void): Promise<UnlistenFn> =>
  listen<Speaker | null>("cast://target", (e) => handler(e.payload));

export const onPlayerEvent = (handler: (event: PlayerEvent) => void): Promise<UnlistenFn> =>
  listen<PlayerEvent>("player://event", (e) => handler(e.payload));
/** A song being fetched before it plays: from iCloud Drive, or from a shared library. */
export const onDownload = (
  handler: (progress: { trackId: number; done: number; total: number; from: string } | null) => void,
): Promise<UnlistenFn> =>
  listen<{ trackId: number; done: number; total: number; from: string } | null>("player://downloading", (e) => handler(e.payload));
export const onScanProgress = (handler: (progress: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>("library://scan-progress", (e) => handler(e.payload));
export const onLibraryChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("library://changed", () => handler());

/** Reads the Music app, or the exported library at `xml`, and says what importing it would find. */
export const importMusicInspect = (xml: string | null) => invoke<ImportPreview>("import_music_inspect", { xml });
/** Imports the library read by `importMusicInspect`, looking in `folder` for the files Music lost. */
export const importMusicRun = (folder: string | null) => invoke<void>("import_music_run", { folder });
/** Lets go of the library `importMusicInspect` read, when the sheet is dismissed. */
export const importMusicCancel = () => invoke<void>("import_music_cancel");
/** Reads the export at `path` (a file, folder or ZIP) from `service`, and says what importing it would find. */
export const elsewhereInspect = (service: ElsewhereService, path: string) => invoke<ElsewherePreview>("elsewhere_inspect", { service, path });
/** Brings in the export read by `elsewhereInspect`: its playlists, and its plays unless `addPlays` is off. */
export const elsewhereRun = (addPlays: boolean) => invoke<void>("elsewhere_run", { addPlays });
/** Lets go of the export `elsewhereInspect` read, when the sheet is dismissed. */
export const elsewhereCancel = () => invoke<void>("elsewhere_cancel");
export const onImportProgress = (handler: (progress: ImportProgress | null) => void): Promise<UnlistenFn> =>
  listen<ImportProgress | null>("import://progress", (e) => handler(e.payload));

export const collectionReport = () => invoke<OwnershipReport | null>("collection_report");
export const collectionLoadMusicApp = () => invoke<void>("collection_load_music_app");
export const collectionLoadXml = () => invoke<void>("collection_load_xml");
export const collectionImportOwned = () => invoke<number>("collection_import_owned");
/** Takes one streamed song off the list; `id` is the `persistentId` of its row. */
export const collectionRemoveStreamed = (id: string) => invoke<void>("collection_remove_streamed", { id });
/** Takes off everything one streaming service's exports put on the list, and says how many entries went. */
export const collectionForgetStreamed = (service: string) => invoke<number>("collection_forget_streamed", { service });
export const collectionLocateFolder = () => invoke<void>("collection_locate_folder");
export const collectionDownloadFromIcloud = () =>
  invoke<{ requested: number; accepted: number }>("collection_download_from_icloud");
/** Carries an error message when reading the other library failed, else null. */
export const onCollectionChanged = (handler: (error: string | null) => void): Promise<UnlistenFn> =>
  listen<string | null>("collection://changed", (e) => handler(e.payload));

export const partyView = () => invoke<PartyView>("party_view");
export const partyPlay = (index: number | null) => invoke<PartyView>("party_play", { index });
export const partySetSettings = (settings: PartySettings) => invoke<PartyView>("party_set_settings", { settings });
/** `at` 0 is "Play Next"; null appends. */
export const partyAdd = (trackIds: number[], at: number | null) => invoke<PartyView>("party_add", { trackIds, at });
export const partyRemove = (positions: number[]) => invoke<PartyView>("party_remove", { positions });
export const partyMove = (positions: number[], to: number) => invoke<PartyView>("party_move", { positions, to });
export const partyReshuffle = () => invoke<PartyView>("party_reshuffle");
export const onPartyChanged = (handler: () => void): Promise<UnlistenFn> => listen("party://changed", () => handler());

export const podcastsFeeds = () => invoke<PodcastFeed[]>("podcasts_feeds");
export const podcastsEpisodes = (feedId: number) => invoke<PodcastEpisode[]>("podcasts_episodes", { feedId });
/** `bulk` is for importing many shows at once, which must not each start a download. */
export const podcastsSubscribe = (url: string, bulk = false) => invoke<PodcastFeed>("podcasts_subscribe", { url, bulk });
export const podcastsDirectoryCategories = () => invoke<PodcastCategory[]>("podcasts_directory_categories");
export const podcastsDirectorySearch = (term: string) => invoke<PodcastListings>("podcasts_directory_search", { term });
export const podcastsDirectoryTop = (category: number | null) => invoke<PodcastListings>("podcasts_directory_top", { category });
export const podcastsAppShows = () => invoke<PodcastListing[]>("podcasts_app_shows");
export const podcastsUnsubscribe = (feedId: number, deleteFiles: boolean) =>
  invoke<void>("podcasts_unsubscribe", { feedId, deleteFiles });
export const podcastsRefresh = (feedId: number | null) => invoke<void>("podcasts_refresh", { feedId });
export const podcastsSetSettings = (feedId: number, settings: FeedSettings) =>
  invoke<void>("podcasts_set_settings", { feedId, settings });
export const podcastsSetPlayed = (episodeId: number, played: boolean) =>
  invoke<void>("podcasts_set_played", { episodeId, played });
export const podcastsDownload = (episodeIds: number[]) => invoke<void>("podcasts_download", { episodeIds });
export const onPodcastsChanged = (handler: () => void): Promise<UnlistenFn> => listen("podcasts://changed", () => handler());
export const onPodcastProgress = (handler: (p: PodcastProgress) => void): Promise<UnlistenFn> =>
  listen<PodcastProgress>("podcasts://progress", (e) => handler(e.payload));

export const purchasesStatus = () => invoke<PurchasesStatus>("purchases_status");
export const purchasesSetWatching = (watching: boolean) => invoke<void>("purchases_set_watching", { watching });
export const purchasesScan = () => invoke<void>("purchases_scan");
export const purchasesAccept = (candidateId: number) => invoke<Purchase>("purchases_accept", { candidateId });
export const purchasesIgnore = (candidateId: number) => invoke<void>("purchases_ignore", { candidateId });
export const purchasesLinks = (artist: string, album: string) => invoke<StoreLink[]>("purchases_links", { artist, album });
export const onPurchasesChanged = (handler: () => void): Promise<UnlistenFn> => listen("purchases://changed", () => handler());

export const updatesStatus = () => invoke<UpdatesStatus>("updates_status");
export const updatesSetAutoCheck = (enabled: boolean) => invoke<void>("updates_set_auto_check", { enabled });
export const updatesDismiss = () => invoke<void>("updates_dismiss");
export const updatesInstall = (onEvent: (event: InstallEvent) => void) => {
  const channel = new Channel<InstallEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("updates_install", { onEvent: channel });
};
export const onUpdateResult = (handler: (result: UpdateResult) => void): Promise<UnlistenFn> =>
  listen<UpdateResult>("updates://result", (e) => handler(e.payload));
export const welcomeDue = () => invoke<boolean>("welcome_due");
/** Records that the welcome was answered, so it doesn't open by itself again. */
export const welcomeDone = () => invoke<void>("welcome_done");
/** The music folders worth offering as a click. Music is checked for holding something; iCloud Drive's only for existing, since looking inside it can raise a macOS privacy prompt. */
export const welcomeFolders = () => invoke<WelcomeFolder[]>("welcome_folders");
export const windowDoubleClickAction = () => invoke<DoubleClickAction>("window_double_click_action");
export const sharingStatus = () => invoke<SharingStatus>("sharing_status");
export const sharingUpdate = (settings: SharingSettings) => invoke<SharingStatus>("sharing_update", { settings });
export const sharingGeneratePassword = () => invoke<string>("sharing_generate_password");
export const visualizerStart = (onFrame: (frame: VisualizerFrame) => void, source: VisualizerSource) => {
  const channel = new Channel<VisualizerFrame>();
  channel.onmessage = onFrame;
  return invoke<void>("visualizer_start", { onFrame: channel, source });
};
export const visualizerStop = () => invoke<void>("visualizer_stop");
export const visualizerMicDevices = () => invoke<MicDevice[]>("visualizer_mic_devices");
/** The analysis stopped by itself, with the reason to show: a microphone that was unplugged. */
export const onVisualizerEnded = (handler: (reason: string) => void): Promise<UnlistenFn> =>
  listen<string>("visualizer://ended", (e) => handler(e.payload));

export const radioGenres = () => invoke<RadioGenre[]>("radio_genres");
export const radioStations = (genre: string, offset: number, limit: number) =>
  invoke<DirectoryStation[]>("radio_stations", { genre, offset, limit });
export const radioSearch = (query: string, limit: number) => invoke<DirectoryStation[]>("radio_search", { query, limit });
export const radioMyStations = () => invoke<RadioStation[]>("radio_my_stations");
export const radioRecent = (limit: number) => invoke<RadioStation[]>("radio_recent", { limit });
export const radioAdd = (station: DirectoryStation) => invoke<number>("radio_add", { station });
export const radioRemove = (id: number) => invoke<void>("radio_remove", { id });
/** The play commands return once the station is chosen; it connects in the background, announced by `onRadioStation`. */
export const radioPlay = (stationId: number) => invoke<void>("radio_play", { stationId });
export const radioPlayDirectory = (station: DirectoryStation) => invoke<void>("radio_play_directory", { station });
export const radioOpenStream = (url: string) => invoke<void>("radio_open_stream", { url });
/** Adds the stations a .pls or .m3u lists; returns their ids. */
export const radioImport = (path: string) => invoke<number[]>("radio_import", { path });
/** The station being tuned in, again when its details change; null once it is removed. */
export const onRadioStation = (handler: (station: RadioStation | null) => void): Promise<UnlistenFn> =>
  listen<RadioStation | null>("radio://station", (e) => handler(e.payload));
export const onRadioLink = (handler: (link: RadioLink | null) => void): Promise<UnlistenFn> =>
  listen<RadioLink | null>("radio://link", (e) => handler(e.payload));
export const onRadioNowPlaying = (handler: (title: RadioTitle) => void): Promise<UnlistenFn> =>
  listen<RadioTitle>("radio://now-playing", (e) => handler(e.payload));

export const onDevicesChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("devices://changed", () => handler());
export const onDeviceSync = (handler: (status: DeviceSyncStatus | null) => void): Promise<UnlistenFn> =>
  listen<DeviceSyncStatus | null>("devices://sync", (e) => handler(e.payload));
export const onCdChanged = (handler: () => void): Promise<UnlistenFn> => listen("cd://changed", () => handler());
export const onCdRip = (handler: (status: CdRipStatus | null) => void): Promise<UnlistenFn> =>
  listen<CdRipStatus | null>("cd://rip", (e) => handler(e.payload));

export type MenuAction =
  | "play-pause"
  | "next"
  | "previous"
  | "next-chapter"
  | "previous-chapter"
  | "volume-up"
  | "volume-down"
  | "new-playlist"
  | "new-playlist-from-selection"
  | "new-smart-playlist"
  | "new-playlist-folder"
  | "get-info"
  | "show-in-finder"
  | "delete"
  | "show-duplicates"
  | "show-exact-duplicates"
  | "organize-library"
  | "set-up-player"
  | "connect-to-server"
  | "clean-up"
  | "get-song-details"
  | "open-stream"
  | "import-music-library"
  | "import-library-xml"
  | "import-elsewhere"
  | "equalizer"
  | "preferences"
  | "welcome"
  | "show-visualizer"
  | `visualizer-${VisualizerStyle}`;
export const onMenuAction = (handler: (action: MenuAction) => void): Promise<UnlistenFn> =>
  listen<MenuAction>("menu://action", (e) => handler(e.payload));
