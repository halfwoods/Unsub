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
  | "size"
  | "playlistOrder";

export interface TrackQuery {
  kind: MediaKind | null;
  search: string | null;
  sort: SortKey;
  descending: boolean;
  /** Restricts the list to one playlist (regular, smart or folder). */
  playlist?: number | null;
  /** Restricts the list to these songs, e.g. Display Duplicates' finds. */
  ids?: number[] | null;
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
  enabled: boolean;
  /** An iCloud Drive placeholder: playing it downloads it first. */
  inCloud: boolean;
  /** Its file has gone; iTunes' "!" in the status column. */
  missing: boolean;
  sortTitle: string | null;
  sortArtist: string | null;
  sortAlbum: string | null;
  sortAlbumArtist: string | null;
  compilation: boolean;
}

/**
 * Get Info's changes: absent fields stay as they are, "" clears text, and null clears a
 * number. Artwork travels separately, as a file path.
 */
export interface TrackEdits {
  title?: string;
  artist?: string;
  albumArtist?: string;
  album?: string;
  genre?: string;
  composer?: string;
  year?: number | null;
  trackNumber?: number | null;
  trackCount?: number | null;
  discNumber?: number | null;
  discCount?: number | null;
  compilation?: boolean;
  sortTitle?: string;
  sortArtist?: string;
  sortAlbum?: string;
  sortAlbumArtist?: string;
  rating?: number;
  enabled?: boolean;
  kind?: MediaKind;
}

export type ArtworkChange = { type: "file"; path: string } | { type: "remove" };

export interface EditReport {
  written: number[];
  /** Songs whose files couldn't take the tags (read-only, in iCloud…): changed in the library only. */
  dbOnly: [number, string][];
  moved: number[];
  moveFailed: [number, string][];
}

export interface MediaSettings {
  folder: string | null;
  organize: boolean;
  copyOnAdd: boolean;
}

export interface OrganizeProgress {
  done: number;
  total: number;
  currentPath: string;
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
  bitPerfect: boolean;
  /** What the device is actually running at, e.g. "24-bit 96 kHz". */
  outputFormat: string | null;
}

export type PlayerEvent =
  | { type: "position"; trackId: number; positionMs: number; durationMs: number }
  | ({ type: "stateChanged" } & PlaybackStatus)
  | { type: "trackStarted"; trackId: number }
  | { type: "trackEnded"; trackId: number }
  | { type: "error"; trackId: number | null; message: string }
  | { type: "notice"; message: string };

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

/** What a non-Apple player plays and where it keeps things; editable in its marker file. */
export interface PlayerProfile {
  musicDir: string;
  playlistDir: string;
  playlistFormat: "m3u8" | "pls";
  /** Playable extensions; the first is what other files become. Empty plays everything. */
  formats: string[];
  maxSampleRate: number | null;
  maxBitDepth: number | null;
  layout: string;
}

/** A Rockbox player, a USB player marked as a sync target, or a phone. */
export interface Player {
  id: string;
  name: string;
  kind: "rockbox" | "massStorage" | "mtp";
  capacityBytes: number;
  freeBytes: number;
  profile: PlayerProfile;
}

export type MusicScope = { type: "all" } | { type: "playlists"; ids: number[] };

/** iTunes' per-iPod sync choices. */
export interface SyncSettings {
  music: boolean;
  musicScope: MusicScope;
  podcasts: boolean;
  podcastsUnplayedOnly: boolean;
  audiobooks: boolean;
  onlyChecked: boolean;
  /** "Convert higher bit rate songs to … AAC"; null keeps lossless songs lossless. */
  convertKbps: number | null;
  /** The first sync to each iPod asks first. */
  confirmed: boolean;
}

export interface SyncPreview {
  tracks: number;
  add: number;
  update: number;
  remove: number;
  bytesNeeded: number;
  freeBytes: number;
  fits: boolean;
  converted: number;
  inCloud: number;
  missing: number;
}

export interface DeviceSyncStatus {
  deviceId: string;
  // iPods report preparing and writingDatabase; players report skipped and playlists.
  stage: "preparing" | "copying" | "writingDatabase" | "cleaning" | "skipped" | "playlists";
  done: number;
  total: number;
  current: string;
}

/** Which slice of a device a list shows: one media kind, or one of its playlists. */
export type DeviceView = { kind: MediaKind } | { playlistId: string };

export type PlaySource =
  | { type: "library" }
  | { type: "device"; deviceId: string }
  | { type: "cd"; discId: string }
  | { type: "shared"; library: string };

/** A library under SHARED: another AntiStreaming Mac found on the network, or a server added by hand. */
export interface SharedLibrary {
  /** "peer:<name>" or "server:<url>". */
  id: string;
  name: string;
  discovered: boolean;
  hasLogin: boolean;
  connected: boolean;
  /** Its songs are still arriving, page by page. */
  loading: boolean;
  songCount: number;
}

export type AccurateRipResult =
  | { result: "matched"; confidence: number }
  | { result: "notInDatabase" }
  | { result: "mismatch" }
  | { result: "skipped" };

export interface CdRelease {
  title: string;
  artist: string;
  year: number | null;
  discNumber: number;
  discCount: number;
}

export type CdLookup = { state: "pending" } | { state: "found" } | { state: "notFound" } | { state: "failed"; message: string };

export interface CdRipStatus {
  discId: string;
  current: number;
  title: string;
  queued: number[];
  fraction: number;
  /** Position of the current song among those being imported, from 0. */
  index: number;
  count: number;
}

/** An audio CD in the drive; its songs load like any list, with track numbers as ids. */
export interface AudioCd {
  id: string;
  name: string;
  trackCount: number;
  lookup: CdLookup;
  releases: CdRelease[];
  chosen: number;
  coverArtUrl: string | null;
  imported: { number: number; accuraterip: AccurateRipResult }[];
  importing: CdRipStatus | null;
}

export type ImportFormat = { codec: "flac" } | { codec: "alac" } | { codec: "aac"; kbps: number };

export type OwnStatus = "inLibrary" | "import" | "locate" | "redownload" | "buy" | "unknown";

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
  totals: { songs: number; inLibrary: number; import: number; locate: number; redownload: number; buy: number; unknown: number };
  missingRoots: { root: string; count: number; available: boolean }[];
  albums: AlbumGroup[];
}

export interface AlbumSummary {
  key: string;
  album: string;
  albumArtist: string;
  year: number | null;
  genre: string;
  trackCount: number;
  durationMs: number;
  artworkTrackId: number;
  dateAdded: number;
  compilation: boolean;
}

export interface GenreSummary {
  name: string;
  albumCount: number;
  trackCount: number;
}

export interface ComposerSummary {
  name: string;
  trackCount: number;
}

export interface ArtistSummary {
  key: string;
  name: string;
  albumCount: number;
  trackCount: number;
}

export type PlaylistKind = "regular" | "smart" | "folder";

export type SmartField =
  | "name" | "artist" | "albumArtist" | "album" | "genre" | "composer" | "kind"
  | "year" | "rating" | "plays" | "skips" | "lastPlayed" | "lastSkipped" | "dateAdded"
  | "time" | "bitRate" | "sampleRate" | "size" | "checked" | "trackNumber" | "discNumber"
  | "compilation" | "playlist";

export type SmartOp =
  | "contains" | "doesNotContain" | "is" | "isNot" | "startsWith" | "endsWith"
  | "greaterThan" | "lessThan" | "inRange" | "before" | "after" | "inTheLast" | "notInTheLast"
  | "isTrue" | "isFalse";

export type DateUnit = "days" | "weeks" | "months";

/** Values use storage units: rating 0–100, time in ms, size in bytes, dates in unix seconds. */
export type SmartValue =
  | { text: string }
  | { number: number }
  | { range: { from: number; to: number } }
  | { relative: { amount: number; unit: DateUnit } }
  | { playlist: number }
  | "empty";

export type SmartRule =
  | { type: "condition"; field: SmartField; op: SmartOp; value: SmartValue }
  | ({ type: "group" } & SmartRules);

export type LimitUnit = "items" | "minutes" | "hours" | "megabytes" | "gigabytes";

export type LimitOrder =
  | "random" | "album" | "artist" | "genre" | "name" | "highestRating" | "lowestRating"
  | "mostRecentlyPlayed" | "leastRecentlyPlayed" | "mostOftenPlayed" | "leastOftenPlayed"
  | "mostRecentlyAdded" | "leastRecentlyAdded";

export interface SmartRules {
  matchAll: boolean;
  rules: SmartRule[];
  limit: { amount: number; unit: LimitUnit; selectedBy: LimitOrder } | null;
  onlyChecked: boolean;
  live: boolean;
}

export interface Playlist {
  id: number;
  name: string;
  kind: PlaylistKind;
  parentId: number | null;
  position: number;
  smart: SmartRules | null;
  trackCount: number;
}

export type PartySource = { type: "library" } | { type: "playlist"; id: number };

export interface PartySettings {
  source: PartySource;
  recent: number;
  upcoming: number;
  favorRated: boolean;
}

export interface PartyView {
  settings: PartySettings;
  history: Track[];
  current: Track | null;
  upcoming: Track[];
}

export interface SharingSettings {
  enabled: boolean;
  port: number;
  username: string;
  password: string;
  /** Converts streams to at most this rate when a phone doesn't ask for one; null sends originals. */
  maxKbps: number | null;
  /** What other AntiStreaming Macs see under SHARED. */
  sharedName: string;
}

export interface SharingStatus {
  settings: SharingSettings;
  running: boolean;
  /** LAN URLs phone apps should use, e.g. http://192.168.1.20:4533. */
  addresses: string[];
  error: string | null;
}

export interface Mix {
  key: string;
  name: string;
  trackIds: number[];
  /** Up to four covers from different albums, for the tile mosaic. */
  artworkTrackIds: number[];
}

export type KeepRule = "all" | { lastN: number } | "unplayed";

export interface FeedSettings {
  autoDownloadLatest: number;
  keep: KeepRule;
}

export interface PodcastFeed {
  id: number;
  url: string;
  title: string;
  author: string;
  description: string;
  artworkUrl: string | null;
  link: string | null;
  lastRefreshed: number | null;
  unplayedCount: number;
  settings: FeedSettings;
}

export interface PodcastEpisode {
  id: number;
  feedId: number;
  title: string;
  /** Raw feed text, often HTML: render as plain text only. */
  description: string;
  published: number | null;
  durationMs: number | null;
  played: boolean;
  positionMs: number;
  file: string | null;
  /** The library track this episode became once downloaded. */
  trackId: number | null;
}

export type PurchaseStore = "bandcamp" | "qobuz" | "sevenDigital" | "hdtracks" | "unknown";

export interface PurchaseCandidate {
  id: number;
  path: string;
  store: PurchaseStore;
  artist: string;
  album: string;
  trackCount: number;
  bytes: number;
  formats: string[];
  foundAt: number;
}

export interface Purchase {
  id: number;
  store: PurchaseStore;
  artist: string;
  album: string;
  files: string[];
  importedAt: number;
  source: string;
}

export interface PurchasesStatus {
  watching: boolean;
  downloads: string;
  candidates: PurchaseCandidate[];
  purchases: Purchase[];
}

export interface StoreLink {
  store: string;
  url: string;
}

export interface UpdatesStatus {
  configured: boolean;
  autoCheck: boolean;
  currentVersion: string;
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
}

export interface UpdateResult {
  manual: boolean;
  update: UpdateInfo | null;
  error: string | null;
}

export type InstallEvent =
  | { event: "started"; data: { contentLength: number | null } }
  | { event: "progress"; data: { chunkLength: number } }
  | { event: "finished" };
