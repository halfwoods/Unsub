import type { SortKey } from "../api/types";

/** Data only: shared by table rendering, view options, persistence, and regression tests. */
export const COLUMN_SPECS = [
  { id: "enabled", title: "Checked", width: 22 },
  { id: "name", title: "Name", width: 240, sort: "name" },
  { id: "time", title: "Time", width: 52, sort: "time" },
  { id: "artist", title: "Artist", width: 170, sort: "artist" },
  { id: "album", title: "Album", width: 170, sort: "album" },
  { id: "genre", title: "Genre", width: 100, sort: "genre" },
  { id: "rating", title: "Rating", width: 74, sort: "rating" },
  { id: "plays", title: "Plays", width: 54, sort: "plays" },
  { id: "year", title: "Year", width: 60, sort: "year" },
  { id: "albumArtist", title: "Album Artist", width: 170, sort: "albumArtist" },
  { id: "composer", title: "Composer", width: 160, sort: "composer" },
  { id: "trackNumber", title: "Track Number", width: 92, sort: "trackNumber" },
  { id: "discNumber", title: "Disc Number", width: 84, sort: "discNumber" },
  { id: "dateAdded", title: "Date Added", width: 154, sort: "dateAdded" },
  { id: "lastPlayed", title: "Last Played", width: 154, sort: "lastPlayed" },
  { id: "skips", title: "Skip Count", width: 78, sort: "skips" },
  { id: "kind", title: "Kind", width: 124, sort: "kind" },
  { id: "bitrate", title: "Bit Rate", width: 82, sort: "bitrate" },
  { id: "sampleRate", title: "Sample Rate", width: 96, sort: "sampleRate" },
  { id: "size", title: "Size", width: 84, sort: "size" },
] as const;

export type ColumnId = (typeof COLUMN_SPECS)[number]["id"];
export type LibraryViewMode = "list" | "albumList" | "grid" | "coverFlow";
export type LibraryGridGroup = "albums" | "artists" | "genres" | "composers";
export interface ColumnLayout {
  order: ColumnId[];
  visible: ColumnId[];
  widths: Partial<Record<ColumnId, number>>;
  sort: SortKey;
  descending: boolean;
}
export interface LibraryViewPrefs {
  version: 1;
  library: ColumnLayout;
  overrides: Record<string, ColumnLayout>;
  presets: Record<string, ColumnLayout>;
  browser: Record<string, boolean>;
  mode: LibraryViewMode;
  group: LibraryGridGroup;
  gridSize: number;
}

export const VIEW_PREFS_KEY = "unsub.libraryViews.v1";
const ids = COLUMN_SPECS.map((c) => c.id);
const sorts: readonly string[] = [...COLUMN_SPECS.flatMap((c) => "sort" in c ? [c.sort] : []), "playlistOrder"];
const isId = (v: unknown): v is ColumnId => typeof v === "string" && (ids as string[]).includes(v);
const object = (v: unknown): Record<string, unknown> => v && typeof v === "object" && !Array.isArray(v) ? v as Record<string, unknown> : {};
export const clampColumnWidth = (width: number, id?: ColumnId) => Math.max(id === "enabled" ? 22 : 28, Math.min(900, Math.round(width)));

export function defaultLayout(): ColumnLayout {
  return { order: [...ids], visible: ids.slice(0, 8), widths: {}, sort: "artist", descending: false };
}

export function normalizeLayout(value: unknown): ColumnLayout {
  const data = object(value);
  const order = Array.isArray(data.order) ? [...new Set(data.order.filter(isId))] : [];
  const visible = Array.isArray(data.visible) ? [...new Set(data.visible.filter(isId))] : defaultLayout().visible;
  if (!visible.includes("name")) visible.unshift("name");
  const widths: ColumnLayout["widths"] = {};
  for (const [id, width] of Object.entries(object(data.widths))) {
    if (isId(id) && typeof width === "number" && Number.isFinite(width)) widths[id] = clampColumnWidth(width, id);
  }
  return {
    order: [...order, ...ids.filter((id) => !order.includes(id))], visible, widths,
    sort: typeof data.sort === "string" && sorts.includes(data.sort) ? data.sort as SortKey : "artist",
    descending: data.descending === true,
  };
}

export function defaultViewPrefs(): LibraryViewPrefs {
  return { version: 1, library: defaultLayout(), overrides: {}, presets: {}, browser: {}, mode: "list", group: "albums", gridSize: 160 };
}

export function parseViewPrefs(raw: string | null): LibraryViewPrefs {
  try {
    const data = object(JSON.parse(raw ?? "{}"));
    // A future incompatible version must not reinterpret the current shape.
    if (data.version !== 1) return defaultViewPrefs();
    const overrides: Record<string, ColumnLayout> = {};
    for (const [key, value] of Object.entries(object(data.overrides))) {
      if (/^(playlist:\d+|device:.+|shared:.+)$/.test(key)) overrides[key] = normalizeLayout(value);
    }
    const presets: Record<string, ColumnLayout> = Object.create(null);
    for (const [name, value] of Object.entries(object(data.presets)).slice(0, 100)) {
      if (name.trim()) presets[name.slice(0, 80)] = normalizeLayout(value);
    }
    const browser: Record<string, boolean> = {};
    for (const [key, value] of Object.entries(object(data.browser))) {
      if (typeof value === "boolean" && /^(music|audiobooks|podcasts|playlist:\d+)$/.test(key)) browser[key] = value;
    }
    return {
      version: 1, library: normalizeLayout(data.library), overrides, presets, browser,
      mode: ["list", "albumList", "grid", "coverFlow"].includes(String(data.mode)) ? data.mode as LibraryViewMode : "list",
      group: ["albums", "artists", "genres", "composers"].includes(String(data.group)) ? data.group as LibraryGridGroup : "albums",
      gridSize: typeof data.gridSize === "number" && Number.isFinite(data.gridSize) ? Math.max(100, Math.min(260, data.gridSize)) : 160,
    };
  } catch { return defaultViewPrefs(); }
}

export function loadViewPrefs(): LibraryViewPrefs {
  try { return parseViewPrefs(localStorage.getItem(VIEW_PREFS_KEY)); } catch { return defaultViewPrefs(); }
}
export function saveViewPrefs(prefs: LibraryViewPrefs): boolean {
  try { localStorage.setItem(VIEW_PREFS_KEY, JSON.stringify(prefs)); return true; } catch { return false; }
}

export function layoutForSource(prefs: LibraryViewPrefs, source: string, regularPlaylist = false): ColumnLayout {
  if (prefs.overrides[source]) return prefs.overrides[source];
  return { ...prefs.library, sort: regularPlaylist ? "playlistOrder" : prefs.library.sort === "playlistOrder" ? "artist" : prefs.library.sort, descending: regularPlaylist ? false : prefs.library.descending };
}
export function withSourceLayout(prefs: LibraryViewPrefs, source: string, layout: ColumnLayout): LibraryViewPrefs {
  const normalized = normalizeLayout(layout);
  if (!/^(playlist|device|shared):/.test(source) && normalized.sort === "playlistOrder") {
    normalized.sort = "artist";
    normalized.descending = false;
  }
  return /^(playlist|device|shared):/.test(source)
    ? { ...prefs, overrides: { ...prefs.overrides, [source]: normalized } }
    : { ...prefs, library: normalized };
}
export function restoreSourceLayout(prefs: LibraryViewPrefs, source: string): LibraryViewPrefs {
  const overrides = { ...prefs.overrides };
  delete overrides[source];
  return /^(playlist|device|shared):/.test(source) ? { ...prefs, overrides } : { ...prefs, library: defaultLayout() };
}
export function moveColumn(layout: ColumnLayout, id: ColumnId, to: ColumnId): ColumnLayout {
  const order = layout.order.filter((column) => column !== id);
  order.splice(Math.max(0, order.indexOf(to)), 0, id);
  return { ...layout, order };
}

const preset = (visible: ColumnId[], sort: SortKey): ColumnLayout => normalizeLayout({ ...defaultLayout(), order: visible, visible, sort });
export const BUILTIN_LAYOUTS: Record<string, ColumnLayout> = {
  "Listening": preset(["enabled", "name", "time", "artist", "album", "rating", "plays"], "artist"),
  "Library Cleanup": preset(["name", "artist", "albumArtist", "album", "year", "genre", "composer", "trackNumber", "discNumber"], "albumArtist"),
  "Audio Quality": preset(["name", "artist", "album", "kind", "bitrate", "sampleRate", "size"], "bitrate"),
};

/** Source-owned fields stay read-only. An iPod's check/rating edits must never target local IDs. */
export function allowedColumn(id: ColumnId, source: string): boolean {
  return !/^(device|shared):/.test(source) || (id !== "enabled" && id !== "rating");
}
