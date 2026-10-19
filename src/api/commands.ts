import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AlbumSummary,
  ArtistSummary,
  ComposerSummary,
  Device,
  DeviceView,
  GenreSummary,
  LibraryStats,
  OwnershipReport,
  PartySettings,
  PartyView,
  PlaybackStatus,
  PlaySource,
  PlayerEvent,
  Playlist,
  PlaylistKind,
  RepeatMode,
  ScanProgress,
  SharingSettings,
  SharingStatus,
  SmartRules,
  Track,
  TrackQuery,
} from "./types";

export const libraryTracks = (query: TrackQuery) => invoke<Track[]>("library_tracks", { query });
export const libraryAlbums = (query: TrackQuery) => invoke<AlbumSummary[]>("library_albums", { query });
export const libraryArtists = (query: TrackQuery) => invoke<ArtistSummary[]>("library_artists", { query });
export const libraryAlbumTracks = (key: string) => invoke<Track[]>("library_album_tracks", { key });
export const libraryGenres = (query: TrackQuery) => invoke<GenreSummary[]>("library_genres", { query });
export const libraryComposers = (query: TrackQuery) => invoke<ComposerSummary[]>("library_composers", { query });
export const libraryPlaylists = () => invoke<Playlist[]>("library_playlists");
export const libraryCreatePlaylist = (name: string, kind: PlaylistKind, parentId: number | null, trackIds: number[] = []) =>
  invoke<Playlist>("library_create_playlist", { name, kind, parentId, trackIds });
export const libraryRenamePlaylist = (id: number, name: string) => invoke<void>("library_rename_playlist", { id, name });
export const libraryDeletePlaylist = (id: number) => invoke<void>("library_delete_playlist", { id });
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
export const libraryStats = (query: TrackQuery) => invoke<LibraryStats>("library_stats", { query });
export const libraryAddPaths = (paths: string[]) => invoke<void>("library_add_paths", { paths });
export const librarySetRating = (trackId: number, rating: number) =>
  invoke<void>("library_set_rating", { trackId, rating });

export const devicesList = () => invoke<Device[]>("devices_list");
export const deviceTracks = (deviceId: string, view: DeviceView, query: TrackQuery) =>
  invoke<Track[]>("device_tracks", { deviceId, view, query });
export const deviceStats = (deviceId: string, view: DeviceView, query: TrackQuery) =>
  invoke<LibraryStats>("device_stats", { deviceId, view, query });
export const deviceEject = (deviceId: string) => invoke<void>("device_eject", { deviceId });

/** Artwork is served by a custom protocol so <img> can cache it without base64 round trips. */
export const artworkUrl = (trackId: number, source: PlaySource = { type: "library" }) =>
  convertFileSrc(source.type === "library" ? String(trackId) : `device/${source.deviceId}/${trackId}`, "artwork");

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

export const onPlayerEvent = (handler: (event: PlayerEvent) => void): Promise<UnlistenFn> =>
  listen<PlayerEvent>("player://event", (e) => handler(e.payload));
export const onScanProgress = (handler: (progress: ScanProgress) => void): Promise<UnlistenFn> =>
  listen<ScanProgress>("library://scan-progress", (e) => handler(e.payload));
export const onLibraryChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("library://changed", () => handler());

export const collectionReport = () => invoke<OwnershipReport | null>("collection_report");
export const collectionLoadMusicApp = () => invoke<void>("collection_load_music_app");
export const collectionLoadXml = () => invoke<void>("collection_load_xml");
export const collectionImportOwned = () => invoke<number>("collection_import_owned");
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

export const sharingStatus = () => invoke<SharingStatus>("sharing_status");
export const sharingUpdate = (settings: SharingSettings) => invoke<SharingStatus>("sharing_update", { settings });

export const onDevicesChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("devices://changed", () => handler());

export type MenuAction =
  | "play-pause"
  | "next"
  | "previous"
  | "volume-up"
  | "volume-down"
  | "new-playlist"
  | "new-playlist-from-selection"
  | "new-smart-playlist"
  | "new-playlist-folder"
  | "preferences";
export const onMenuAction = (handler: (action: MenuAction) => void): Promise<UnlistenFn> =>
  listen<MenuAction>("menu://action", (e) => handler(e.payload));
