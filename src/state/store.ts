import { create } from "zustand";
import { orderAlbumTracks, remapTrackAnchor } from "../browser/albumKeys";
import { layoutFor, isLayoutSource } from "../preferences/libraryViews";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { browseQuery, browserQueries, EMPTY_BROWSER, narrowBrowser, playlistPositionQuery, withoutBrowse, type BrowseStep, type BrowserFilters } from "../browser/browseScope";
import { loadVisualizerPrefs, saveVisualizerPrefs, type VisualizerPrefs } from "../preferences/playbackPrefs";
import { applyTextSizes, listRowHeight, loadTextSizes, saveTextSizes, type TextSizes } from "../preferences/textSizes";
import * as api from "../api/commands";
import type {
  AlbumSummary,
  ArtistSummary,
  AudioCd,
  CdRipStatus,
  Chapter,
  ComposerSummary,
  Device,
  DeviceSyncStatus,
  Player,
  SharedLibrary,
  Speaker,
  DeviceView,
  GenreSummary,
  ImportPreview,
  ImportProgress,
  LibraryStats,
  MediaKind,
  OrganizeProgress,
  CleanUpProgress,
  DetailsProgress,
  PlaybackStatus,
  PlaySource,
  PlayerEvent,
  PodcastEpisode,
  PodcastProgress,
  Playlist,
  PlaylistKind,
  RadioLink,
  RadioStation,
  RadioTitle,
  RepeatMode,
  ScanProgress,
  SmartRules,
  SortKey,
  Track,
  TrackQuery,
} from "../api/types";

export type ViewMode = "list" | "albumList" | "grid" | "coverFlow";

/** Album List renders consecutive album groups and therefore needs an album-compatible sort. */
const sortForView = (sort: SortKey, descending: boolean, view: ViewMode) =>
  view === "albumList" && !["album", "albumArtist", "artist"].includes(sort) ? { sort: "album" as SortKey, descending: false } : { sort, descending };

/** The grid's Albums / Artists / Genres / Composers switch. */
export type GridGroup = "albums" | "artists" | "genres" | "composers";

export type LibrarySource = "music" | "podcasts" | "audiobooks";

/** Sources whose features land in later milestones; they show an iTunes-style intro placard. */
export type PlaylistSourceId = `playlist:${number}`;

/**
 * "device:<id>" is a device's Summary page; "device:<id>:<kind>" and
 * "device:<id>:playlist:<playlistId>" are lists of what's on it.
 */
export type DeviceSourceId = `device:${string}`;

/** "cd:<MusicBrainz DiscID>": an audio CD's songs. */
export type CdSourceId = `cd:${string}`;

/** "player:<id>": a non-Apple player's sync page. */
export type PlayerSourceId = `player:${string}`;

/** "shared:<library id>": a library under SHARED. */
export type SharedSourceId = `shared:${string}`;

export type SourceId =
  | LibrarySource
  | DeviceSourceId
  | CdSourceId
  | PlayerSourceId
  | SharedSourceId
  | PlaylistSourceId
  | "toOwn"
  | "partyShuffle"
  | "mixes"
  | "radio"
  | "inbox"
  | "purchased";

const SOURCE_KIND: Record<LibrarySource, MediaKind> = {
  music: "music",
  podcasts: "podcast",
  audiobooks: "audiobook",
};

export const isLibrarySource = (source: SourceId): source is LibrarySource => source in SOURCE_KIND;

export function deviceSource(deviceId: string, view?: DeviceView): DeviceSourceId {
  if (!view) return `device:${deviceId}`;
  return "kind" in view ? `device:${deviceId}:${view.kind}` : `device:${deviceId}:playlist:${view.playlistId}`;
}

export const isDeviceSource = (source: SourceId): source is DeviceSourceId => source.startsWith("device:");

export const cdSource = (discId: string): CdSourceId => `cd:${discId}`;
export const playerSource = (id: string): PlayerSourceId => `player:${id}`;
export const sharedSource = (id: string): SharedSourceId => `shared:${id}`;
export const sharedIdOf = (source: SourceId) => (source.startsWith("shared:") ? source.slice("shared:".length) : null);
export const playerIdOf = (source: SourceId) => (source.startsWith("player:") ? source.slice("player:".length) : null);
export const cdIdOf = (source: SourceId) => (source.startsWith("cd:") ? source.slice("cd:".length) : null);

export const isPlaylistSource = (source: SourceId): source is PlaylistSourceId => source.startsWith("playlist:");
export const playlistSource = (id: number): PlaylistSourceId => `playlist:${id}`;
export const playlistIdOf = (source: SourceId) => (isPlaylistSource(source) ? Number(source.slice("playlist:".length)) : null);

/** Sheets over the library: Get Info, Clean Up Song Info, a missing file to locate, a delete to confirm, Organize Library, Get Song Details from iCloud. */
export type LibrarySheet =
  | { type: "info"; trackIds: number[] }
  | { type: "cleanUp"; trackIds: number[] }
  | { type: "locate"; track: Track }
  | { type: "delete"; trackIds: number[] }
  | { type: "organize" }
  | { type: "songDetails" };

export interface SmartEditor {
  /** null while creating a new smart playlist. */
  id: number | null;
  name: string;
  rules: SmartRules;
}

export function parseDeviceSource(source: SourceId): { deviceId: string; view: DeviceView | null } | null {
  if (!isDeviceSource(source)) return null;
  const [, deviceId, part, playlistId] = source.split(":");
  if (part === undefined) return { deviceId, view: null };
  if (part === "playlist") return { deviceId, view: { playlistId } };
  return { deviceId, view: { kind: part as MediaKind } };
}

export const sameSource = (a: PlaySource, b: PlaySource) => {
  if (a.type === "device") return b.type === "device" && a.deviceId === b.deviceId;
  if (a.type === "cd") return b.type === "cd" && a.discId === b.discId;
  if (a.type === "shared") return b.type === "shared" && a.library === b.library;
  return b.type === "library";
};

/** Where the rows on screen come from, which is also where playback must resolve them. */
export function playSourceOf(source: SourceId): PlaySource {
  const device = parseDeviceSource(source);
  if (device) return { type: "device", deviceId: device.deviceId };
  const discId = cdIdOf(source);
  if (discId !== null) return { type: "cd", discId };
  const library = sharedIdOf(source);
  return library === null ? { type: "library" } : { type: "shared", library };
}

/** iTunes restarts the current chapter instead of going back once you're this far in. */
const PREVIOUS_RESTARTS_AFTER_MS = 3000;

/** The chapter playing at `positionMs`, or -1 before the first one starts. */
export function chapterIndexAt(chapters: readonly Chapter[], positionMs: number): number {
  const after = chapters.findIndex((c) => c.startMs > positionMs);
  return (after === -1 ? chapters.length : after) - 1;
}

/** Where Next or Previous Chapter seeks to from `positionMs`, or null when there is no such chapter. */
export function chapterSeekTarget(chapters: readonly Chapter[], positionMs: number, direction: "next" | "previous"): number | null {
  const current = chapterIndexAt(chapters, positionMs);
  if (direction === "next") return chapters[current + 1]?.startMs ?? null;
  if (current < 0) return null;
  const restart = positionMs - chapters[current].startMs > PREVIOUS_RESTARTS_AFTER_MS;
  return chapters[restart ? current : Math.max(0, current - 1)].startMs;
}

/** Totals for lists the backend doesn't count: CDs and shared libraries. */
function totals(tracks: Track[]): LibraryStats {
  return {
    count: tracks.length,
    totalDurationMs: tracks.reduce((sum, t) => sum + t.durationMs, 0),
    totalBytes: tracks.reduce((sum, t) => sum + t.fileSize, 0),
  };
}

const IDLE_PLAYBACK: PlaybackStatus = {
  state: "stopped",
  trackId: null,
  positionMs: 0,
  durationMs: 0,
  sampleRate: null,
  outputSampleRate: null,
  volume: 0.75,
  bitPerfect: false,
  outputFormat: null,
  live: false,
  buffering: false,
};

/** How long a player notice stays in the LCD before the song info returns. */
const NOTICE_MS = 4000;

interface SelectModifiers {
  extend: boolean;
  toggle: boolean;
}

interface AppState {
  windowActive: boolean;
  /** Native full screen, which the player can enter without the visualizer. */
  windowFullscreen: boolean;
  source: SourceId;
  viewMode: ViewMode;
  query: TrackQuery;
  loadedQuery: TrackQuery | null;
  tracks: Track[];
  stats: LibraryStats | null;
  selection: ReadonlySet<number>;
  selectionAnchor: number | null;
  /** A one-shot reveal, separate from the anchor so repeated Command-L presses scroll again. */
  songReveal: { source: SourceId; query: TrackQuery; trackId: number } | null;
  playback: PlaybackStatus;
  nowPlaying: Track | null;
  /** The speaker the music is playing on; null for this computer. */
  castTarget: Speaker | null;
  /** The playing library song's chapters, tagged with the song so a slow read can't land on the next one. */
  chapters: { trackId: number; list: Chapter[] } | null;
  shuffle: boolean;
  repeat: RepeatMode;
  scan: ScanProgress | null;
  showArtwork: boolean;
  devices: Device[];
  players: Player[];
  shared: SharedLibrary[];
  cds: AudioCd[];
  /** The CD import in progress, if any. */
  cdRip: CdRipStatus | null;
  /** The iPod sync in progress, if any. */
  deviceSync: DeviceSyncStatus | null;
  librarySheet: LibrarySheet | null;
  /** Display Duplicates is on, showing only songs that have duplicates. */
  duplicates: { exact: boolean } | null;
  organize: OrganizeProgress | null;
  /** Clean Up Song Info's progress while it runs. */
  cleanUp: CleanUpProgress | null;
  /** Get Song Details from iCloud's progress while it runs, in the background. */
  details: DetailsProgress | null;
  /** The Music library File › Import Music Library has read, shown in the sheet that asks where its files are. */
  importSheet: ImportPreview | null;
  importProgress: ImportProgress | null;
  settingUpPlayer: boolean;
  /** The iPod whose Copy to Library sheet is open. */
  copyFromDevice: string | null;
  videosFromDevice: string | null;
  connectingToServer: boolean;
  openingStream: boolean;
  importingElsewhere: boolean;
  /** The welcome sheet: it opens by itself on a first run, and Help › Welcome to Unsub… opens it again. */
  welcoming: boolean;
  /** The welcome started an import; its "You're all set" tips wait for the first songs to arrive. */
  welcomeTipPending: boolean;
  /** The station being listened to, kept once paused so Play can tune it in again; a song replaces it. */
  liveStation: RadioStation | null;
  liveTitle: RadioTitle | null;
  liveLink: RadioLink | null;
  equalizerOpen: boolean;
  /** `open` is whether the visualizer replaces the window's body; the rest persists. */
  visualizer: VisualizerPrefs & { open: boolean };
  expandedDevices: ReadonlySet<string>;
  /** Snapshot of the list playback started from, so now-playing info survives navigating away. */
  playingTracks: Track[];
  playingSource: PlaySource;
  albums: AlbumSummary[];
  artists: ArtistSummary[];
  genres: GenreSummary[];
  composers: ComposerSummary[];
  gridGroup: GridGroup;
  browsePath: BrowseStep[];
  browseRootSort: { sort: SortKey; descending: boolean } | null;
  browserFilters: BrowserFilters;
  browserFacets: { genres: GenreSummary[]; artists: ArtistSummary[]; albums: AlbumSummary[] };
  browseScroll: Record<string, number>;
  playlists: Playlist[];
  expandedFolders: ReadonlySet<number>;
  renamingPlaylist: number | null;
  smartEditor: SmartEditor | null;
  preferencesOpen: boolean;
  notice: string | null;
  /** A song being fetched before it plays, from iCloud Drive or a shared library. */
  download: { trackId: number; from: string; fraction: number } | null;
  /**
   * Podcast episodes asked for or on their way, until each lands or fails: the title for the LCD, whether the download has
   * started (the backend has begun it, rather than it waiting its turn), and how far along (null until the size is known).
   */
  podcastDownloads: Record<number, { title: string; started: boolean; fraction: number | null }>;
  /**
   * The episode to play as soon as its download lands. Only the listener choosing something else drops it (a song or station
   * played, Next or Previous, another episode asked for): a song ending on its own, pausing, stopping or resuming isn't a
   * choice, and the double-click that asked for the episode meant "play this now".
   */
  podcastWish: number | null;
  textSizes: TextSizes;
  rowHeight: number;

  setWindowActive: (active: boolean) => void;
  setWindowFullscreen: (fullscreen: boolean) => void;
  setSource: (source: SourceId) => void;
  setViewMode: (mode: ViewMode) => void;
  setSearch: (search: string) => void;
  sortBy: (key: SortKey) => void;
  refresh: () => Promise<void>;
  selectRow: (index: number, modifiers: SelectModifiers) => void;
  playRow: (index: number) => Promise<void>;
  revealCurrentSong: (canReveal?: () => boolean) => Promise<void>;
  togglePlay: () => Promise<void>;
  setVolume: (volume: number) => void;
  seekTo: (positionMs: number) => void;
  toggleShuffle: () => void;
  cycleRepeat: () => void;
  setScan: (scan: ScanProgress | null) => void;
  toggleArtwork: () => void;
  refreshDevices: () => Promise<void>;
  refreshCds: () => Promise<void>;
  refreshShared: () => Promise<void>;
  openLibrarySheet: (sheet: LibrarySheet | null) => void;
  /**
   * Delete for the selected songs: in a regular playlist it removes them from the playlist only; elsewhere in
   * the library it asks before deleting them. `fromLibrary` asks even in a playlist.
   */
  deleteSelection: (fromLibrary?: boolean) => Promise<void>;
  /** Reads the Music app (or the exported library at `xml`), then opens the import sheet. */
  openImportSheet: (xml: string | null) => Promise<boolean>;
  showDuplicates: (exact: boolean) => Promise<void>;
  showAllSongs: () => void;
  toggleDevice: (deviceId: string) => void;
  setGridGroup: (group: GridGroup) => void;
  showMusicView: (view: "songs" | "albums" | "artists") => void;
  openArtist: (key: string, label: string) => void;
  openAlbum: (key: string, label: string) => void;
  openGroup: (kind: "genre" | "composer", key: string, label: string) => void;
  browseBack: () => void;
  browseTo: (index: number) => void;
  setBrowseScroll: (key: string, offset: number) => void;
  setBrowserFilter: (field: keyof BrowserFilters, value: string | null) => void;
  clearBrowser: () => void;
  toggleColumnBrowser: () => void;
  playBrowse: (shuffled?: boolean) => Promise<void>;
  refreshPlaylists: () => Promise<void>;
  toggleFolder: (id: number) => void;
  /** `parentId` makes it a child of that folder. */
  createPlaylist: (kind: Exclude<PlaylistKind, "smart">, fromSelection?: boolean, parentId?: number | null) => Promise<void>;
  setRenamingPlaylist: (id: number | null) => void;
  openSmartEditor: (editor: SmartEditor | null) => void;
  setPreferencesOpen: (open: boolean) => void;
  setVisualizer: (changes: Partial<VisualizerPrefs & { open: boolean }>) => void;
  setTextSizes: (sizes: TextSizes) => void;
  playAlbum: (key: string) => Promise<void>;
  /** `shuffled` turns shuffle on and starts anywhere in the list. */
  playTrackIds: (trackIds: number[], shuffled?: boolean) => Promise<void>;
  applyPlayerEvent: (event: PlayerEvent) => void;
  /** Shows a message in the LCD for a few seconds. */
  showNotice: (message: string) => void;
  /** Downloads the episodes that aren't on their way already. */
  downloadPodcasts: (episodes: PodcastEpisode[]) => void;
  /** For an episode that isn't downloaded: fetches it, and plays it once it lands. */
  playPodcastWhenDownloaded: (episode: PodcastEpisode) => void;
  applyPodcastProgress: (progress: PodcastProgress) => void;
}

const NEXT_REPEAT: Record<RepeatMode, RepeatMode> = { off: "all", all: "one", one: "off" };

function withoutKey<T>(record: Record<number, T>, key: number): Record<number, T> {
  const { [key]: _removed, ...rest } = record;
  return rest;
}

export const useApp = create<AppState>((set, get) => ({
  windowActive: true,
  windowFullscreen: false,
  source: "music",
  viewMode: useLibraryViews.getState().prefs.browsing.viewMode,
  query: { kind: "music", search: null, ...sortForView(layoutFor(useLibraryViews.getState().prefs, "music").sort, layoutFor(useLibraryViews.getState().prefs, "music").descending, useLibraryViews.getState().prefs.browsing.viewMode) },
  loadedQuery: null,
  tracks: [],
  stats: null,
  selection: new Set(),
  selectionAnchor: null,
  songReveal: null,
  playback: IDLE_PLAYBACK,
  nowPlaying: null,
  castTarget: null,
  chapters: null,
  shuffle: false,
  repeat: "off",
  scan: null,
  showArtwork: false,
  devices: [],
  players: [],
  shared: [],
  cds: [],
  cdRip: null,
  deviceSync: null,
  librarySheet: null,
  duplicates: null,
  organize: null,
  cleanUp: null,
  details: null,
  importSheet: null,
  importProgress: null,
  settingUpPlayer: false,
  copyFromDevice: null,
  videosFromDevice: null,
  connectingToServer: false,
  openingStream: false,
  importingElsewhere: false,
  welcoming: false,
  welcomeTipPending: false,
  liveStation: null,
  liveTitle: null,
  liveLink: null,
  equalizerOpen: false,
  visualizer: { ...loadVisualizerPrefs(), open: false },
  expandedDevices: new Set(),
  playingTracks: [],
  playingSource: { type: "library" },
  albums: [],
  artists: [],
  genres: [],
  composers: [],
  gridGroup: useLibraryViews.getState().prefs.browsing.gridGroup,
  browsePath: [],
  browseRootSort: null,
  browserFilters: { ...EMPTY_BROWSER },
  browserFacets: { genres: [], artists: [], albums: [] },
  browseScroll: {},
  playlists: [],
  expandedFolders: new Set(),
  renamingPlaylist: null,
  smartEditor: null,
  preferencesOpen: false,
  notice: null,
  download: null,
  podcastDownloads: {},
  podcastWish: null,
  textSizes: loadTextSizes(),
  rowHeight: listRowHeight(loadTextSizes()),

  setWindowActive: (windowActive) => set({ windowActive }),
  setWindowFullscreen: (windowFullscreen) => set({ windowFullscreen }),

  setSource: (source) => {
    const device = parseDeviceSource(source);
    const playlist = playlistIdOf(source);
    const isList = isLibrarySource(source) || device?.view != null || playlist !== null || cdIdOf(source) !== null || sharedIdOf(source) !== null;
    // Device and playlist lists filter on the backend, so the query's own kind stays unset.
    const kind = isLibrarySource(source) ? SOURCE_KIND[source] : null;
    const regular = get().playlists.find((p) => p.id === playlist)?.kind === "regular";
    const layout = isLayoutSource(source) ? layoutFor(useLibraryViews.getState().prefs, source, regular) : null;
    set((s) => ({
      source,
      songReveal: null,
      query: { ...withoutBrowse(s.query), kind, search: null, playlist, ids: null, ...sortForView(layout?.sort ?? "artist", layout?.descending ?? false, s.viewMode) },
      browsePath: [],
      browseRootSort: null,
      browserFilters: { ...EMPTY_BROWSER },
      browserFacets: { genres: [], artists: [], albums: [] },
      duplicates: null,
      tracks: isList ? s.tracks : [],
      stats: isList ? s.stats : null,
      selection: new Set(),
      selectionAnchor: null,
    }));
    if (isList) void get().refresh();
    // A shared library with a saved login opens on selection, as iTunes' did.
    const shared = sharedIdOf(source);
    const library = get().shared.find((l) => l.id === shared);
    if (library?.hasLogin && !library.connected) void api.remoteOpen(library.id).catch((e: unknown) => get().showNotice(String(e)));
  },

  // Album List only makes sense in album order, so iTunes switches the sort to it.
  setViewMode: (viewMode) => {
    const { query, viewMode: previousView, source, playlists, tracks: previousTracks, selectionAnchor } = get();
    const saved = layoutFor(useLibraryViews.getState().prefs, source, playlists.some(p => p.id === playlistIdOf(source) && p.kind === "regular"));
    const grouped = query.sort === "album" || query.sort === "albumArtist" || query.sort === "artist";
    const tracks = viewMode === "albumList" ? orderAlbumTracks(previousTracks) : previousTracks;
    set({ viewMode, tracks, selectionAnchor: remapTrackAnchor(previousTracks, tracks, selectionAnchor), songReveal: null, query: viewMode === "albumList" && !grouped ? { ...query, sort: "album", descending: false }
      : previousView === "albumList" && viewMode !== "albumList" ? { ...query, sort: saved.sort, descending: saved.descending } : query });
    useLibraryViews.getState().setBrowsing({ viewMode });
    void get().refresh();
  },

  setSearch: (search) => {
    const value = search.trim() === "" ? null : search;
    if (get().query.search === value) return;
    set((s) => ({ query: { ...s.query, search: value }, selection: new Set(), selectionAnchor: null, songReveal: null }));
    void get().refresh();
  },

  // iTunes: clicking the sorted column flips direction; clicking another column sorts ascending.
  sortBy: (sort) => {
    set((s) => ({
      query: { ...s.query, sort, descending: s.query.sort === sort ? !s.query.descending : false },
      selectionAnchor: null,
    }));
    const { source, query, playlists } = get();
    if (isLayoutSource(source) && get().viewMode !== "albumList") useLibraryViews.getState().setSort(source, sort, query.descending, playlists.some((p) => p.id === playlistIdOf(source) && p.kind === "regular"));
    void get().refresh();
  },

  refresh: async () => {
    const { query, source, tracks: previousTracks, selectionAnchor: previousAnchor, selection: previousSelection } = get();
    // Never offer old-scope rows for playback or edits while a narrower request is pending.
    if (get().loadedQuery !== query) set({ tracks: [], stats: null, albums: [], artists: [], genres: [], composers: [] });
    const device = parseDeviceSource(source);
    const discId = cdIdOf(source);
    const sharedId = sharedIdOf(source);
    let request: Promise<[Track[], LibraryStats]>;
    if (isLibrarySource(source) || isPlaylistSource(source)) {
      request = Promise.all([api.libraryTracks(query), api.libraryStats(query)]);
    } else if (device?.view) {
      const { deviceId, view } = device;
      request = Promise.all([api.deviceTracks(deviceId, view, query), api.deviceStats(deviceId, view, query)]);
    } else if (sharedId !== null) {
      request = api.remoteTracks(sharedId, query).then((tracks): [Track[], LibraryStats] => [tracks, totals(tracks)]);
    } else if (discId !== null) {
      // A CD is a dozen songs in disc order, so its totals are summed here.
      request = api.cdTracks(discId).then((tracks): [Track[], LibraryStats] => [tracks, totals(tracks)]);
    } else {
      return;
    }
    // Album views need album rows too; iPod lists stay song-only until device albums exist.
    const { viewMode, gridGroup } = get();
    const wantsAlbums = (isLibrarySource(source) || isPlaylistSource(source)) && (viewMode === "grid" || viewMode === "coverFlow");
    const grid = wantsAlbums && viewMode === "grid";
    const browserVisible = useLibraryViews.getState().prefs.browsing.browserVisible && (isLibrarySource(source) || isPlaylistSource(source));
    const facets = browserQueries(query);
    try {
    const [[tracks, stats], albums, artists, genres, composers, browserFacets] = await Promise.all([
      request,
      wantsAlbums ? (query.artistKey != null ? api.libraryArtistAlbums(query.artistKey, query) : api.libraryAlbums(query)) : Promise.resolve([]),
      grid && gridGroup === "artists" ? api.libraryArtists(query) : Promise.resolve([]),
      grid && gridGroup === "genres" ? api.libraryGenres(query) : Promise.resolve([]),
      grid && gridGroup === "composers" ? api.libraryComposers(query) : Promise.resolve([]),
      browserVisible ? Promise.all([api.libraryGenres(facets.genres), api.libraryArtists(facets.artists), api.libraryAlbums(facets.albums)])
        .then(([genres, artists, albums]) => ({ genres, artists, albums })) : Promise.resolve({ genres: [], artists: [], albums: [] }),
    ]);
    // Drop stale responses when the user typed or switched source mid-flight.
    if (get().query !== query || get().source !== source || get().viewMode !== viewMode || get().gridGroup !== gridGroup) return;
    const nextTracks = viewMode === "albumList" ? orderAlbumTracks(tracks) : tracks;
    const current = get();
    // A query change temporarily removes rows. Keep its captured occurrence only if
    // selection has not changed meanwhile; explicit anchor resets must stay reset.
    const anchorTracks = current.tracks.length ? current.tracks
      : current.selectionAnchor === previousAnchor && current.selection === previousSelection ? previousTracks : [];
    set({ tracks: nextTracks, selectionAnchor: remapTrackAnchor(anchorTracks, nextTracks, current.selectionAnchor), stats, albums, artists, genres, composers, browserFacets, loadedQuery: query });
    } catch (error) {
      if (get().query === query && get().source === source) get().showNotice(`Couldn't load library: ${String(error)}`);
    }
  },

  selectRow: (index, { extend, toggle }) => {
    const { tracks, selection, selectionAnchor } = get();
    const id = tracks[index]?.id;
    if (id === undefined) return;
    if (extend && selectionAnchor !== null) {
      const [from, to] = [Math.min(selectionAnchor, index), Math.max(selectionAnchor, index)];
      set({ selection: new Set(tracks.slice(from, to + 1).map((t) => t.id)) });
    } else if (toggle) {
      const next = new Set(selection);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      set({ selection: next, selectionAnchor: index });
    } else {
      set({ selection: new Set([id]), selectionAnchor: index });
    }
  },

  playRow: async (index) => {
    const { tracks, source } = get();
    if (index < 0 || index >= tracks.length) return;
    // iTunes' "The song could not be used because the original file could not be found."
    if (tracks[index].missing) return set({ librarySheet: { type: "locate", track: tracks[index] } });
    const playingSource = playSourceOf(source);
    // iTunes skips unchecked songs when playing through a list, but plays one you double-click.
    // A station in the list plays alone; it can't queue behind songs.
    const queue = tracks.filter((t, i) => i === index || (t.enabled && t.kind !== "radio"));
    set({ playingTracks: tracks, playingSource, podcastWish: null });
    await api.playerPlayList(
      playingSource,
      queue.map((t) => t.id),
      tracks.slice(0, index).filter(t => t.enabled && t.kind !== "radio").length,
    );
  },

  revealCurrentSong: async (canReveal = () => true) => {
    const initial = get(), trackId = initial.playback.trackId, playingSource = initial.playingSource;
    if (!canReveal() || trackId === null || initial.playback.live || initial.playback.state === "stopped") return;
    let expectedSource = initial.source, expectedQuery = initial.query, expectedView = initial.viewMode;
    const current = () => canReveal() && get().source === expectedSource && get().query === expectedQuery && get().viewMode === expectedView
      && get().playback.trackId === trackId && !get().playback.live && get().playback.state !== "stopped" && sameSource(get().playingSource, playingSource);
    const unavailable = () => get().showNotice("The current song is no longer available in its source.");
    // CD, device and shared IDs must never be looked up in the local library.
    if ((playingSource.type === "device" && !initial.devices.some(d => d.id === playingSource.deviceId))
      || (playingSource.type === "cd" && !initial.cds.some(d => d.id === playingSource.discId))
      || (playingSource.type === "shared" && !initial.shared.some(l => l.id === playingSource.library && l.hasLogin))) return unavailable();
    const select = () => {
      const state = get();
      const index = state.selectionAnchor !== null && state.tracks[state.selectionAnchor]?.id === trackId
        ? state.selectionAnchor : state.tracks.findIndex(t => t.id === trackId);
      if (index < 0) return unavailable();
      set({ viewMode: "list", visualizer: { ...state.visualizer, open: false }, selection: new Set([trackId]), selectionAnchor: index,
        songReveal: { source: state.source, query: state.query, trackId } });
      useLibraryViews.getState().setBrowsing({ viewMode: "list" });
    };
    const isListSource = (source: SourceId) => source === "music" || source === "audiobooks" || isPlaylistSource(source)
      || parseDeviceSource(source)?.view != null || cdIdOf(source) !== null || sharedIdOf(source) !== null;
    const compatible = isListSource(initial.source) && sameSource(playSourceOf(initial.source), playingSource);
    if (compatible && initial.loadedQuery === initial.query && initial.tracks.some(t => t.id === trackId)) { select(); return; }
    // Membership probes are bounded to this ID where the source supports it; refresh loads the final full list once.
    const find = async (source: SourceId, query: TrackQuery) => {
      const probe = { ...query, ids: [trackId] }, device = parseDeviceSource(source);
      const rows = device?.view ? await api.deviceTracks(device.deviceId, device.view, probe)
        : playingSource.type === "cd" ? await api.cdTracks(playingSource.discId)
        : playingSource.type === "shared" ? await api.remoteTracks(playingSource.library, probe)
        : await api.libraryTracks(probe);
      return rows.find(t => t.id === trackId);
    };
    try {
      let source = initial.source;
      let query: TrackQuery = { ...withoutBrowse(initial.query), search: null, ids: null, ...(initial.browseRootSort ?? {}) };
      let found = compatible ? await find(source, query) : undefined;
      if (!current()) return;
      if (!found) {
        let track = initial.playingTracks.find(t => t.id === trackId) ?? (initial.nowPlaying?.id === trackId ? initial.nowPlaying : null);
        if (!track && playingSource.type === "library") track = await api.libraryTrack(trackId);
        if (!current()) return;
        if (playingSource.type === "library") {
          if (!track) return unavailable();
          if (track.kind !== "music" && track.kind !== "audiobook") {
            get().showNotice("This item is not in a song list. Open Podcasts or Radio to find it."); return;
          }
          source = track.kind === "audiobook" ? "audiobooks" : "music";
        } else if (playingSource.type === "device") source = deviceSource(playingSource.deviceId, { kind: track?.kind ?? "music" });
        else if (playingSource.type === "cd") source = cdSource(playingSource.discId);
        else source = sharedSource(playingSource.library);
        const layout = layoutFor(useLibraryViews.getState().prefs, source);
        query = { ...withoutBrowse(initial.query), kind: isLibrarySource(source) ? SOURCE_KIND[source] : null, playlist: null,
          search: null, ids: null, sort: layout.sort, descending: layout.descending };
        found = await find(source, query);
        if (!current()) return;
        if (!found) return unavailable();
      }
      expectedSource = source; expectedQuery = query; expectedView = "list";
      set({ source, query, viewMode: "list", songReveal: null, selection: new Set(), selectionAnchor: null,
        browsePath: [], browseRootSort: null, browserFilters: { ...EMPTY_BROWSER }, browserFacets: { genres: [], artists: [], albums: [] }, duplicates: null,
        expandedDevices: playingSource.type === "device" ? new Set([...get().expandedDevices, playingSource.deviceId]) : get().expandedDevices });
      await get().refresh();
      if (current() && get().loadedQuery === query) select();
    } catch (error) {
      if (current()) get().showNotice(`Couldn't show the current song: ${String(error)}`);
    }
  },

  // iTunes' play button with nothing loaded plays the selection, else the top of the list.
  togglePlay: async () => {
    const { playback, tracks, selection, source } = get();
    if (playback.trackId !== null || get().liveStation !== null) return api.playerTogglePause();
    // In Party Shuffle, Play starts the party rather than the (empty) song list.
    if (source === "partyShuffle") {
      set({ playingSource: { type: "library" }, podcastWish: null });
      await api.partyPlay(null);
      return;
    }
    const selected = tracks.findIndex((t) => selection.has(t.id));
    await get().playRow(Math.max(selected, 0));
  },

  setVolume: (volume) => {
    set((s) => ({ playback: { ...s.playback, volume } }));
    void api.playerSetVolume(volume);
  },

  // The engine reports no position while paused, so the LCD and the next chapter step start from here.
  seekTo: (positionMs) => {
    set((s) => ({ playback: { ...s.playback, positionMs } }));
    void api.playerSeek(positionMs);
  },

  toggleShuffle: () => {
    const shuffle = !get().shuffle;
    set({ shuffle });
    void api.playerSetShuffle(shuffle);
  },

  cycleRepeat: () => {
    const repeat = NEXT_REPEAT[get().repeat];
    set({ repeat });
    void api.playerSetRepeat(repeat);
  },

  setScan: (scan) => set({ scan }),

  toggleArtwork: () => set((s) => ({ showArtwork: !s.showArtwork })),

  refreshDevices: async () => {
    const [devices, players] = await Promise.all([api.devicesList(), api.playersList()]);
    // The sheet goes with its iPod, which must not bring it back when plugged in again.
    set((s) => ({ devices, players, copyFromDevice: devices.some((d) => d.id === s.copyFromDevice) ? s.copyFromDevice : null,
      videosFromDevice: devices.some((d) => d.id === s.videosFromDevice) ? s.videosFromDevice : null }));
    // Like iTunes, unplugging the iPod you're looking at drops you back to Music.
    const device = parseDeviceSource(get().source);
    const player = playerIdOf(get().source);
    if ((device && !devices.some((d) => d.id === device.deviceId)) || (player !== null && !players.some((p) => p.id === player))) {
      get().setSource("music");
    }
  },

  openLibrarySheet: (librarySheet) => set({ librarySheet }),

  deleteSelection: async (fromLibrary = false) => {
    const { source, selection, query, playlists, viewMode } = get();
    // The album grid doesn't show which songs are selected, so Delete there must not act on them.
    if (selection.size === 0 || viewMode === "grid") return;
    const playlistId = playlistIdOf(source);
    if (!fromLibrary && playlistId !== null && playlists.some((p) => p.id === playlistId && p.kind === "regular")) {
      // A playlist may repeat a song, so its rows go by position in its stored order, which the list may be sorted or filtered away from.
      const stored = await api.libraryTracks(playlistPositionQuery(query));
      await api.libraryRemoveFromPlaylist(
        playlistId,
        stored.flatMap((t, i) => (selection.has(t.id) ? [i] : [])),
      );
      await Promise.all([get().refresh(), get().refreshPlaylists()]);
    } else if (isLibrarySource(source) || isPlaylistSource(source)) {
      set({ librarySheet: { type: "delete", trackIds: [...selection] } });
    }
  },

  openImportSheet: async (xml) => {
    try {
      set({ importSheet: await api.importMusicInspect(xml) });
      return true;
    } catch (e) {
      get().showNotice(String(e));
      return false;
    }
  },

  showDuplicates: async (exact) => {
    const ids = (await api.libraryDuplicates(exact)).flat();
    if (ids.length === 0) return get().showNotice(exact ? "No exact duplicates found" : "No duplicates found");
    if (!isLibrarySource(get().source)) get().setSource("music");
    // Sorted by name so each set of duplicates sits together.
    set((s) => ({ duplicates: { exact }, query: { ...s.query, ids, search: null, sort: "name", descending: false }, selection: new Set() }));
    void get().refresh();
  },

  showAllSongs: () => {
    set((s) => ({ duplicates: null, query: { ...s.query, ids: null } }));
    void get().refresh();
  },

  refreshShared: async () => {
    const shared = await api.remoteLibraries();
    set({ shared });
    const id = sharedIdOf(get().source);
    if (id === null) return;
    // A library that left the network drops you back to Music; one still loading fills in.
    if (shared.some((l) => l.id === id)) void get().refresh();
    else get().setSource("music");
  },

  refreshCds: async () => {
    const cds = await api.cdList();
    set({ cds });
    const discId = cdIdOf(get().source);
    if (discId === null) return;
    // Ejecting the CD you're looking at drops you back to Music; otherwise its names or
    // checkboxes may have changed, e.g. MusicBrainz just answered.
    if (cds.some((cd) => cd.id === discId)) void get().refresh();
    else get().setSource("music");
  },

  setGridGroup: (gridGroup) => {
    set((s) => ({ gridGroup, browsePath: [], browseRootSort: null, query: { ...browseQuery(s.query, s.browserFilters, []), ...(s.browseRootSort ?? {}) }, selection: new Set(), selectionAnchor: null }));
    useLibraryViews.getState().setBrowsing({ gridGroup });
    void get().refresh();
  },

  showMusicView: (view) => {
    if (get().source !== "music") get().setSource("music");
    const viewMode = view === "songs" ? "list" : "grid";
    const gridGroup = view === "artists" ? "artists" : "albums";
    const layout = layoutFor(useLibraryViews.getState().prefs, "music");
    set((s) => ({ viewMode, gridGroup, browsePath: [], browseRootSort: null, query: { ...browseQuery(s.query, s.browserFilters, []), sort: layout.sort, descending: layout.descending }, selection: new Set(), selectionAnchor: null }));
    useLibraryViews.getState().setBrowsing({ viewMode, gridGroup });
    void get().refresh();
  },

  openArtist: (key, label) => {
    const browsePath: BrowseStep[] = [{ kind: "artist", key, label }];
    set((s) => ({ browsePath, browseRootSort: s.browseRootSort ?? { sort: s.query.sort, descending: s.query.descending }, viewMode: "grid", gridGroup: "albums", query: browseQuery(s.query, s.browserFilters, browsePath), selection: new Set(), selectionAnchor: null }));
    void get().refresh();
  },

  openGroup: (kind, key, label) => {
    const browsePath: BrowseStep[] = [{ kind, key, label }];
    set((s) => ({ browsePath, browseRootSort: s.browseRootSort ?? { sort: s.query.sort, descending: s.query.descending }, viewMode: "grid", gridGroup: "albums", query: browseQuery(s.query, s.browserFilters, browsePath), selection: new Set(), selectionAnchor: null }));
    void get().refresh();
  },

  openAlbum: (key, label) => {
    const browsePath: BrowseStep[] = [...get().browsePath.filter((p) => p.kind !== "album"), { kind: "album", key, label }];
    set((s) => ({ browsePath, browseRootSort: s.browseRootSort ?? { sort: s.query.sort, descending: s.query.descending }, viewMode: "list", query: { ...browseQuery(s.query, s.browserFilters, browsePath), sort: "album", descending: false }, selection: new Set(), selectionAnchor: null }));
    void get().refresh();
  },

  browseBack: () => get().browseTo(get().browsePath.length - 2),
  browseTo: (index) => {
    const previous = get().browsePath;
    const browsePath = previous.slice(0, Math.max(0, index + 1));
    const root = previous[0]?.kind;
    const gridGroup = browsePath.length ? "albums" : root === "artist" ? "artists" : root === "genre" ? "genres" : root === "composer" ? "composers" : "albums";
    set((s) => ({ browsePath, browseRootSort: browsePath.length ? s.browseRootSort : null, viewMode: "grid", gridGroup,
      query: { ...browseQuery(s.query, s.browserFilters, browsePath), ...(s.browseRootSort ?? { sort: "artist", descending: false }) }, selection: new Set(), selectionAnchor: null }));
    void get().refresh();
  },

  setBrowseScroll: (key, offset) => set((s) => ({ browseScroll: { ...Object.fromEntries(Object.entries(s.browseScroll).slice(-99)), [key]: offset } })),
  setBrowserFilter: (field, value) => {
    const browserFilters = narrowBrowser(get().browserFilters, field, value);
    set((s) => ({ browserFilters, browsePath: [], browseRootSort: null, viewMode: "list", query: browseQuery(s.query, browserFilters, []), selection: new Set(), selectionAnchor: null }));
    void get().refresh();
  },
  clearBrowser: () => {
    set((s) => ({ browserFilters: { ...EMPTY_BROWSER }, browsePath: [], browseRootSort: null, query: { ...withoutBrowse(s.query), ...(s.browseRootSort ?? {}) }, selection: new Set(), selectionAnchor: null }));
    void get().refresh();
  },
  toggleColumnBrowser: () => {
    if (!isLibrarySource(get().source) && !isPlaylistSource(get().source)) return;
    const browserVisible = !useLibraryViews.getState().prefs.browsing.browserVisible;
    useLibraryViews.getState().setBrowsing({ browserVisible, ...(browserVisible ? { viewMode: "list" as const } : {}) });
    if (!browserVisible) get().clearBrowser();
    else { set({ viewMode: "list" }); void get().refresh(); }
  },
  playBrowse: async (shuffled = false) => {
    const tracks = get().tracks.filter((t) => t.enabled && t.kind !== "radio");
    if (!tracks.length) return;
    if (get().shuffle !== shuffled) get().toggleShuffle();
    set({ playingTracks: tracks, playingSource: { type: "library" }, podcastWish: null });
    await api.playerPlayList({ type: "library" }, tracks.map((t) => t.id), shuffled ? Math.floor(Math.random() * tracks.length) : 0);
  },

  // Mixes arrive as ids; playing them works like any library list.
  playTrackIds: async (trackIds, shuffled = false) => {
    if (trackIds.length === 0) return;
    // Shuffle through the engine's own mode, so the Shuffle button tells the truth and turning it off
    // brings the list's real order back; the engine keeps the first song and shuffles the rest.
    if (shuffled && !get().shuffle) get().toggleShuffle();
    // The LCD names a song from the list playback started from while it downloads from iCloud.
    const tracks = await api.libraryTracks({ kind: null, search: null, sort: "name", descending: false, ids: trackIds });
    set({ playingTracks: tracks, playingSource: { type: "library" }, podcastWish: null });
    await api.playerPlayList({ type: "library" }, trackIds, shuffled ? Math.floor(Math.random() * trackIds.length) : 0);
  },

  playAlbum: async (key) => {
    const tracks = (await api.libraryAlbumTracks(key, get().query)).filter((t) => t.enabled && t.kind !== "radio");
    if (tracks.length === 0) return;
    set({ playingTracks: tracks, playingSource: { type: "library" }, podcastWish: null });
    await api.playerPlayList({ type: "library" }, tracks.map((t) => t.id), 0);
  },

  refreshPlaylists: async () => set({ playlists: await api.libraryPlaylists() }),

  toggleFolder: (id) =>
    set((s) => {
      const expanded = new Set(s.expandedFolders);
      if (expanded.has(id)) expanded.delete(id);
      else expanded.add(id);
      return { expandedFolders: expanded };
    }),

  // Like iTunes: the new playlist is selected and its name is ready to type over.
  createPlaylist: async (kind, fromSelection = false, parentId = null) => {
    const { selection, source } = get();
    const trackIds = fromSelection && !isDeviceSource(source) ? [...selection] : [];
    const name = kind === "folder" ? "untitled folder" : "untitled playlist";
    const playlist = await api.libraryCreatePlaylist(name, kind, parentId, trackIds);
    await get().refreshPlaylists();
    // A collapsed folder would hide the new row, and the name field with it.
    if (parentId !== null && !get().expandedFolders.has(parentId)) get().toggleFolder(parentId);
    get().setSource(playlistSource(playlist.id));
    set({ renamingPlaylist: playlist.id });
  },

  setRenamingPlaylist: (renamingPlaylist) => set({ renamingPlaylist }),

  openSmartEditor: (smartEditor) => set({ smartEditor }),

  setPreferencesOpen: (preferencesOpen) => set({ preferencesOpen }),

  setVisualizer: (changes) => {
    const { open, ...prefs } = { ...get().visualizer, ...changes };
    saveVisualizerPrefs(prefs);
    set({ visualizer: { ...prefs, open } });
  },

  setTextSizes: (textSizes) => {
    saveTextSizes(textSizes);
    applyTextSizes(textSizes);
    set({ textSizes, rowHeight: listRowHeight(textSizes) });
  },

  toggleDevice: (deviceId) =>
    set((s) => {
      const expanded = new Set(s.expandedDevices);
      if (expanded.has(deviceId)) expanded.delete(deviceId);
      else expanded.add(deviceId);
      return { expandedDevices: expanded };
    }),

  applyPlayerEvent: (event) => {
    switch (event.type) {
      case "position":
        set((s) => ({
          playback: { ...s.playback, positionMs: event.positionMs, durationMs: event.durationMs },
        }));
        break;
      case "stateChanged": {
        const { type: _type, ...status } = event;
        set({ playback: status });
        if (status.trackId === null) set({ nowPlaying: null, chapters: null });
        // Tuning in again starts without a title until the station announces one.
        if (!status.live) set({ liveTitle: null });
        break;
      }
      case "trackStarted": {
        const source = get().playingSource;
        const known = get().playingTracks.find((t) => t.id === event.trackId) ?? null;
        // Gapless playback starts the next track without changing play/pause state.
        // The list's speaker follows playback.trackId, so move it on this event too.
        set((s) => ({
          playback: { ...s.playback, trackId: event.trackId, positionMs: 0, durationMs: known?.durationMs ?? 0 },
          nowPlaying: known,
          liveStation: null,
          liveLink: null,
          chapters: { trackId: event.trackId, list: [] },
        }));
        if (!known) void api.libraryTrack(event.trackId).then((t) => {
          if (get().playback.trackId === event.trackId && sameSource(get().playingSource, source)) set({ nowPlaying: t });
        });
        if (source.type === "library") {
          void api.libraryChapters(event.trackId).then((list) => {
            // A CTOC keeps the file's own order, which needn't be time order.
            if (get().chapters?.trackId === event.trackId) set({ chapters: { trackId: event.trackId, list: [...list].sort((a, b) => a.startMs - b.startMs) } });
          });
        }
        break;
      }
      case "trackEnded":
        // Play counts change on the backend; refresh so the Plays column stays truthful.
        void get().refresh();
        break;
      case "error":
        console.error("[player]", event.message);
        break;
      case "notice":
        get().showNotice(event.message);
        break;
    }
  },

  showNotice: (message) => {
    set({ notice: message });
    setTimeout(() => get().notice === message && set({ notice: null }), NOTICE_MS);
  },

  downloadPodcasts: (episodes) => {
    const fresh = episodes.filter((e) => !(e.id in get().podcastDownloads));
    if (fresh.length === 0) return;
    set((s) => ({
      podcastDownloads: { ...s.podcastDownloads, ...Object.fromEntries(fresh.map((e) => [e.id, { title: e.title, started: false, fraction: null }])) },
    }));
    void api.podcastsDownload(fresh.map((e) => e.id));
  },

  // Asked again while the episode is already on its way, it still means "play it when it lands".
  playPodcastWhenDownloaded: (episode) => {
    set({ podcastWish: episode.id });
    get().downloadPodcasts([episode]);
  },

  applyPodcastProgress: (progress) => {
    switch (progress.state) {
      case "downloading": {
        const fraction = progress.total ? progress.done / progress.total : null;
        set((s) => ({ podcastDownloads: { ...s.podcastDownloads, [progress.episodeId]: { title: progress.title, started: true, fraction } } }));
        break;
      }
      case "done": {
        set((s) => ({ podcastDownloads: withoutKey(s.podcastDownloads, progress.episodeId) }));
        // Playing it is a choice like any other, which is what clears the wish.
        if (get().podcastWish === progress.episodeId) void get().playTrackIds([progress.trackId]);
        break;
      }
      case "failed":
        set((s) => ({
          podcastDownloads: withoutKey(s.podcastDownloads, progress.episodeId),
          podcastWish: s.podcastWish === progress.episodeId ? null : s.podcastWish,
        }));
        break;
    }
  },
}));

// Presets, Restore Defaults and explicit layout copies can change the current sort.
// Width/visibility-only updates never refetch a large library.
useLibraryViews.subscribe((next, previous) => {
  const { source, playlists, query } = useApp.getState();
  if (!isLayoutSource(source)) return;
  const regular = playlists.some((p) => p.id === playlistIdOf(source) && p.kind === "regular");
  const before = layoutFor(previous.prefs, source, regular);
  const after = layoutFor(next.prefs, source, regular);
  if ((before.sort !== after.sort || before.descending !== after.descending)
      && (query.sort !== after.sort || query.descending !== after.descending)) {
    useApp.setState({ query: { ...query, ...sortForView(after.sort, after.descending, useApp.getState().viewMode) }, selectionAnchor: null });
    void useApp.getState().refresh();
  }
});
