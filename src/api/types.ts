// Mirrors the serde shapes in crates/library and crates/playback (camelCase on the wire).

export type MediaKind = "music" | "podcast" | "audiobook";

export type SortKey =
  | "name"
  | "time"
  | "artist"
  | "album"
  | "albumArtist"
  | "genre"
  | "year"
  | "rating"
  | "plays"
  | "lastPlayed"
  | "dateAdded"
  | "kind"
  | "bitrate"
  | "sampleRate"
  | "size";

export interface TrackQuery {
  kind: MediaKind | null;
  search: string | null;
  sort: SortKey;
  descending: boolean;
}

export interface Track {
  id: number;
  path: string;
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  genre: string;
  composer: string;
  year: number | null;
  trackNumber: number | null;
  trackCount: number | null;
  discNumber: number | null;
  discCount: number | null;
  durationMs: number;
  bitrateKbps: number | null;
  sampleRate: number | null;
  bitDepth: number | null;
  channels: number | null;
  codec: string;
  fileSize: number;
  dateAdded: number;
  dateModified: number;
  playCount: number;
  lastPlayed: number | null;
  skipCount: number;
  rating: number;
  kind: MediaKind;
  compilation: boolean;
}

export interface LibraryStats {
  count: number;
  totalDurationMs: number;
  totalBytes: number;
}

export interface ScanProgress {
  scanned: number;
  added: number;
  currentPath: string;
}

export type PlaybackState = "stopped" | "playing" | "paused";

export interface PlaybackStatus {
  state: PlaybackState;
  trackId: number | null;
  positionMs: number;
  durationMs: number;
  sampleRate: number | null;
  outputSampleRate: number | null;
  volume: number;
}

export type PlayerEvent =
  | { type: "position"; trackId: number; positionMs: number; durationMs: number }
  | ({ type: "stateChanged" } & PlaybackStatus)
  | { type: "trackStarted"; trackId: number }
  | { type: "trackEnded"; trackId: number }
  | { type: "error"; trackId: number | null; message: string };

export type RepeatMode = "off" | "all" | "one";

export type IpodFamily = "original" | "mini" | "photo" | "video" | "classic" | "nano" | "shuffle" | "unknown";

export interface IpodModel {
  family: IpodFamily;
  generation: number;
  name: string;
  capacityGb: number | null;
  color: string | null;
  modelNumber: string;
}

export interface DeviceUsage {
  audioBytes: number;
  videoBytes: number;
  podcastBytes: number;
  audiobookBytes: number;
}

export interface DevicePlaylist {
  /** 64-bit iTunesDB ids exceed JS's safe integers, so they travel as strings. */
  id: string;
  name: string;
  isPodcasts: boolean;
  isSmart: boolean;
  trackCount: number;
}

export type SyncSupport = "supported" | "needsHash58" | "needsHash72" | "unsupported";

export interface Device {
  id: string;
  name: string;
  model: IpodModel | null;
  capacityBytes: number;
  freeBytes: number;
  serialNumber: string | null;
  sync: SyncSupport;
  usage: DeviceUsage;
  counts: { music: number; podcast: number; audiobook: number };
  playlists: DevicePlaylist[];
}

/** Which slice of a device a list shows: one media kind, or one of its playlists. */
export type DeviceView = { kind: MediaKind } | { playlistId: string };

export type PlaySource = { type: "library" } | { type: "device"; deviceId: string };

export type OwnStatus = "inLibrary" | "import" | "redownload" | "buy" | "unknown";

export interface ReportTrack {
  persistentId: string;
  title: string;
  artist: string;
  durationMs: number | null;
  trackNumber: number | null;
  discNumber: number | null;
  kind: string;
  status: OwnStatus;
  trackId: number | null;
  matchQuality: "sameFile" | "exact" | "close" | null;
}

export interface AlbumGroup {
  albumArtist: string;
  album: string;
  tracks: ReportTrack[];
}

export interface OwnershipReport {
  source: string;
  loadedAt: number;
  totals: { songs: number; inLibrary: number; import: number; redownload: number; buy: number; unknown: number };
  albums: AlbumGroup[];
}
