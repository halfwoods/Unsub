import { create } from "zustand";
import { applyTextSizes, listRowHeight, loadTextSizes, saveTextSizes, type TextSizes } from "../preferences/textSizes";
import * as api from "../api/commands";
import type {
  AlbumSummary,
  ArtistSummary,
  ComposerSummary,
  Device,
  DeviceView,
  GenreSummary,
  LibraryStats,
  MediaKind,
  PlaybackStatus,
  PlaySource,
  PlayerEvent,
  Playlist,
  PlaylistKind,
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

export type SourceId =
  | LibrarySource
  | DeviceSourceId
  | PlaylistSourceId
  | "toOwn"
  | "partyShuffle"
  | "mixes"
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

export const isPlaylistSource = (source: SourceId): source is PlaylistSourceId => source.startsWith("playlist:");
export const playlistSource = (id: number): PlaylistSourceId => `playlist:${id}`;
export const playlistIdOf = (source: SourceId) => (isPlaylistSource(source) ? Number(source.slice("playlist:".length)) : null);

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

export const sameSource = (a: PlaySource, b: PlaySource) =>
  a.type === b.type && (a.type === "library" || (b.type === "device" && a.deviceId === b.deviceId));

/** Where the rows on screen come from, which is also where playback must resolve them. */
export function playSourceOf(source: SourceId): PlaySource {
  const device = parseDeviceSource(source);
  return device ? { type: "device", deviceId: device.deviceId } : { type: "library" };
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
  shuffle: boolean;
  repeat: RepeatMode;
  scan: ScanProgress | null;
  showArtwork: boolean;
  devices: Device[];
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
  /** A cloud-only song being fetched before it plays. */
  cloudDownload: { trackId: number; fraction: number } | null;
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
  toggleShuffle: () => void;
  cycleRepeat: () => void;
  setScan: (scan: ScanProgress | null) => void;
  toggleArtwork: () => void;
  refreshDevices: () => Promise<void>;
  toggleDevice: (deviceId: string) => void;
  setGridGroup: (group: GridGroup) => void;
  refreshPlaylists: () => Promise<void>;
  toggleFolder: (id: number) => void;
  createPlaylist: (kind: Exclude<PlaylistKind, "smart">, fromSelection?: boolean) => Promise<void>;
  setRenamingPlaylist: (id: number | null) => void;
  openSmartEditor: (editor: SmartEditor | null) => void;
  setPreferencesOpen: (open: boolean) => void;
  setTextSizes: (sizes: TextSizes) => void;
  playAlbum: (key: string) => Promise<void>;
  playTrackIds: (trackIds: number[]) => Promise<void>;
  applyPlayerEvent: (event: PlayerEvent) => void;
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
  shuffle: false,
  repeat: "off",
  scan: null,
  showArtwork: false,
  devices: [],
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
  cloudDownload: null,
  textSizes: loadTextSizes(),
  rowHeight: listRowHeight(loadTextSizes()),

  setWindowActive: (windowActive) => set({ windowActive }),

  setSource: (source) => {
    const device = parseDeviceSource(source);
    const playlist = playlistIdOf(source);
    const isList = isLibrarySource(source) || device?.view != null || playlist !== null;
    // Device and playlist lists filter on the backend, so the query's own kind stays unset.
    const kind = isLibrarySource(source) ? SOURCE_KIND[source] : null;
    const regular = get().playlists.find((p) => p.id === playlist)?.kind === "regular";
    // A regular playlist opens in its own order (the # column); everything else sorts like the library.
    const sortFor = (current: SortKey): SortKey => (regular ? "playlistOrder" : current === "playlistOrder" ? "artist" : current);
    set((s) => ({
      source,
      query: { ...s.query, kind, search: null, playlist, sort: sortFor(s.query.sort), descending: regular ? false : s.query.descending },
      tracks: isList ? s.tracks : [],
      stats: isList ? s.stats : null,
      selection: new Set(),
      selectionAnchor: null,
    }));
    if (isList) void get().refresh();
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
    let request: Promise<[Track[], LibraryStats]>;
    if (isLibrarySource(source) || isPlaylistSource(source)) {
      request = Promise.all([api.libraryTracks(query), api.libraryStats(query)]);
    } else if (device?.view) {
      const { deviceId, view } = device;
      request = Promise.all([api.deviceTracks(deviceId, view, query), api.deviceStats(deviceId, view, query)]);
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
    const playingSource = playSourceOf(source);
    // iTunes skips unchecked songs when playing through a list, but plays one you double-click.
    const queue = tracks.filter((t, i) => t.enabled || i === index);
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
    if (playback.trackId !== null) return api.playerTogglePause();
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
    const devices = await api.devicesList();
    set({ devices });
    // Like iTunes, unplugging the iPod you're looking at drops you back to Music.
    const device = parseDeviceSource(get().source);
    if (device && !devices.some((d) => d.id === device.deviceId)) get().setSource("music");
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
        if (status.trackId === null) set({ nowPlaying: null });
        break;
      }
      case "trackStarted": {
        const known = get().playingTracks.find((t) => t.id === event.trackId);
        if (known) set({ nowPlaying: known });
        else void api.libraryTrack(event.trackId).then((t) => set({ nowPlaying: t }));
        break;
      }
      case "trackEnded":
        // Play counts change on the backend; refresh so the Plays column stays truthful.
        void get().refresh();
        break;
      case "error":
        console.error("[player]", event.message);
        break;
      case "notice": {
        set({ notice: event.message });
        setTimeout(() => get().notice === event.message && set({ notice: null }), NOTICE_MS);
        break;
      }
    }
  },
}));
