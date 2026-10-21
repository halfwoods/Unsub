import { create } from "zustand";
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
  source: SourceId;
  viewMode: ViewMode;
  query: TrackQuery;
  tracks: Track[];
  stats: LibraryStats | null;
  selection: ReadonlySet<number>;
  selectionAnchor: number | null;
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
  playlists: Playlist[];
  expandedFolders: ReadonlySet<number>;
  renamingPlaylist: number | null;
  smartEditor: SmartEditor | null;
  preferencesOpen: boolean;
  notice: string | null;
  /** A song being fetched before it plays, from iCloud Drive or a shared library. */
  download: { trackId: number; from: string; fraction: number } | null;
  textSizes: TextSizes;
  rowHeight: number;

  setWindowActive: (active: boolean) => void;
  setSource: (source: SourceId) => void;
  setViewMode: (mode: ViewMode) => void;
  setSearch: (search: string) => void;
  sortBy: (key: SortKey) => void;
  refresh: () => Promise<void>;
  selectRow: (index: number, modifiers: SelectModifiers) => void;
  playRow: (index: number) => Promise<void>;
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
  openImportSheet: (xml: string | null) => Promise<void>;
  showDuplicates: (exact: boolean) => Promise<void>;
  showAllSongs: () => void;
  toggleDevice: (deviceId: string) => void;
  setGridGroup: (group: GridGroup) => void;
  refreshPlaylists: () => Promise<void>;
  toggleFolder: (id: number) => void;
  createPlaylist: (kind: Exclude<PlaylistKind, "smart">, fromSelection?: boolean) => Promise<void>;
  setRenamingPlaylist: (id: number | null) => void;
  openSmartEditor: (editor: SmartEditor | null) => void;
  setPreferencesOpen: (open: boolean) => void;
  setVisualizer: (changes: Partial<VisualizerPrefs & { open: boolean }>) => void;
  setTextSizes: (sizes: TextSizes) => void;
  playAlbum: (key: string) => Promise<void>;
  playTrackIds: (trackIds: number[]) => Promise<void>;
  applyPlayerEvent: (event: PlayerEvent) => void;
  /** Shows a message in the LCD for a few seconds. */
  showNotice: (message: string) => void;
}

const NEXT_REPEAT: Record<RepeatMode, RepeatMode> = { off: "all", all: "one", one: "off" };

export const useApp = create<AppState>((set, get) => ({
  windowActive: true,
  source: "music",
  viewMode: "list",
  query: { kind: "music", search: null, sort: "artist", descending: false },
  tracks: [],
  stats: null,
  selection: new Set(),
  selectionAnchor: null,
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
  gridGroup: "albums",
  playlists: [],
  expandedFolders: new Set(),
  renamingPlaylist: null,
  smartEditor: null,
  preferencesOpen: false,
  notice: null,
  download: null,
  textSizes: loadTextSizes(),
  rowHeight: listRowHeight(loadTextSizes()),

  setWindowActive: (windowActive) => set({ windowActive }),

  setSource: (source) => {
    const device = parseDeviceSource(source);
    const playlist = playlistIdOf(source);
    const isList = isLibrarySource(source) || device?.view != null || playlist !== null || cdIdOf(source) !== null || sharedIdOf(source) !== null;
    // Device and playlist lists filter on the backend, so the query's own kind stays unset.
    const kind = isLibrarySource(source) ? SOURCE_KIND[source] : null;
    const regular = get().playlists.find((p) => p.id === playlist)?.kind === "regular";
    // A regular playlist opens in its own order (the # column); everything else sorts like the library.
    const sortFor = (current: SortKey): SortKey => (regular ? "playlistOrder" : current === "playlistOrder" ? "artist" : current);
    set((s) => ({
      source,
      query: { ...s.query, kind, search: null, playlist, ids: null, sort: sortFor(s.query.sort), descending: regular ? false : s.query.descending },
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
    const { query } = get();
    const grouped = query.sort === "album" || query.sort === "albumArtist" || query.sort === "artist";
    set({ viewMode, query: viewMode === "albumList" && !grouped ? { ...query, sort: "album", descending: false } : query });
    void get().refresh();
  },

  setSearch: (search) => {
    set((s) => ({ query: { ...s.query, search: search.trim() === "" ? null : search } }));
    void get().refresh();
  },

  // iTunes: clicking the sorted column flips direction; clicking another column sorts ascending.
  sortBy: (sort) => {
    set((s) => ({
      query: { ...s.query, sort, descending: s.query.sort === sort ? !s.query.descending : false },
    }));
    void get().refresh();
  },

  refresh: async () => {
    const { query, source } = get();
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
    const [[tracks, stats], albums, artists, genres, composers] = await Promise.all([
      request,
      wantsAlbums ? api.libraryAlbums(query) : Promise.resolve([]),
      grid && gridGroup === "artists" ? api.libraryArtists(query) : Promise.resolve([]),
      grid && gridGroup === "genres" ? api.libraryGenres(query) : Promise.resolve([]),
      grid && gridGroup === "composers" ? api.libraryComposers(query) : Promise.resolve([]),
    ]);
    // Drop stale responses when the user typed or switched source mid-flight.
    if (get().query !== query || get().source !== source) return;
    set({ tracks, stats, albums, artists, genres, composers });
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
    set({ playingTracks: tracks, playingSource });
    await api.playerPlayList(
      playingSource,
      queue.map((t) => t.id),
      queue.indexOf(tracks[index]),
    );
  },

  // iTunes' play button with nothing loaded plays the selection, else the top of the list.
  togglePlay: async () => {
    const { playback, tracks, selection, source } = get();
    if (playback.trackId !== null || get().liveStation !== null) return api.playerTogglePause();
    // In Party Shuffle, Play starts the party rather than the (empty) song list.
    if (source === "partyShuffle") {
      set({ playingSource: { type: "library" } });
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
    set({ devices, players });
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
      const stored = await api.libraryTracks({ ...query, sort: "playlistOrder", descending: false, search: null });
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
    } catch (e) {
      get().showNotice(String(e));
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
    set({ gridGroup });
    void get().refresh();
  },

  // Mixes arrive as ids; playing them works like any library list.
  playTrackIds: async (trackIds) => {
    if (trackIds.length === 0) return;
    set({ playingTracks: [], playingSource: { type: "library" } });
    await api.playerPlayList({ type: "library" }, trackIds, 0);
  },

  playAlbum: async (key) => {
    const tracks = await api.libraryAlbumTracks(key);
    if (tracks.length === 0) return;
    set({ playingTracks: tracks, playingSource: { type: "library" } });
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
  createPlaylist: async (kind, fromSelection = false) => {
    const { selection, source } = get();
    const trackIds = fromSelection && !isDeviceSource(source) ? [...selection] : [];
    const name = kind === "folder" ? "untitled folder" : "untitled playlist";
    const playlist = await api.libraryCreatePlaylist(name, kind, null, trackIds);
    await get().refreshPlaylists();
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
        set({ liveStation: null, liveLink: null });
        const known = get().playingTracks.find((t) => t.id === event.trackId);
        if (known) set({ nowPlaying: known });
        else void api.libraryTrack(event.trackId).then((t) => set({ nowPlaying: t }));
        set({ chapters: { trackId: event.trackId, list: [] } });
        if (get().playingSource.type === "library") {
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
}));
