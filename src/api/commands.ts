import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AlbumSummary,
  ArtistSummary,
  Device,
  DeviceView,
  LibraryStats,
  OwnershipReport,
  PlaybackStatus,
  PlaySource,
  PlayerEvent,
  RepeatMode,
  ScanProgress,
  Track,
  TrackQuery,
} from "./types";

export const libraryTracks = (query: TrackQuery) => invoke<Track[]>("library_tracks", { query });
export const libraryAlbums = (query: TrackQuery) => invoke<AlbumSummary[]>("library_albums", { query });
export const libraryArtists = (query: TrackQuery) => invoke<ArtistSummary[]>("library_artists", { query });
export const libraryAlbumTracks = (key: string) => invoke<Track[]>("library_album_tracks", { key });
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
/** Carries an error message when reading the other library failed, else null. */
export const onCollectionChanged = (handler: (error: string | null) => void): Promise<UnlistenFn> =>
  listen<string | null>("collection://changed", (e) => handler(e.payload));

export const onDevicesChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("devices://changed", () => handler());

export type MenuAction = "play-pause" | "next" | "previous" | "volume-up" | "volume-down";
export const onMenuAction = (handler: (action: MenuAction) => void): Promise<UnlistenFn> =>
  listen<MenuAction>("menu://action", (e) => handler(e.payload));
