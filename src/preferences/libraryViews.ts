import type { SortKey } from "../api/types";

export const LIBRARY_VIEWS_KEY = "unsub.libraryViews";
export const COLUMN_DEFAULTS = [
  ["enabled", 22], ["name", 240], ["time", 52], ["artist", 170], ["album", 170], ["genre", 100], ["rating", 74], ["plays", 52],
  ["year", 62], ["albumArtist", 170], ["composer", 160], ["trackNumber", 90], ["discNumber", 90], ["dateAdded", 148],
  ["lastPlayed", 148], ["skipCount", 78], ["kind", 100], ["bitrate", 85], ["sampleRate", 100], ["size", 90],
] as const;
export type ColumnId = typeof COLUMN_DEFAULTS[number][0];
export interface ColumnLayout {
  order: ColumnId[];
  visible: ColumnId[];
  widths: Partial<Record<ColumnId, number>>;
  sort: SortKey;
  descending: boolean;
}
export interface BrowsingPreferences {
  browserVisible: boolean;
  viewMode: "list" | "albumList" | "grid" | "coverFlow";
  gridGroup: "albums" | "artists" | "genres" | "composers";
}
export interface LibraryViewPrefs {
  version: 1;
  libraryDefault: ColumnLayout;
  playlists: Record<string, ColumnLayout>;
  customPresets: { name: string; layout: ColumnLayout }[];
  browsing: BrowsingPreferences;
}
export const DEFAULT_LAYOUT: ColumnLayout = {
  order: COLUMN_DEFAULTS.map(([id]) => id), visible: COLUMN_DEFAULTS.slice(0, 8).map(([id]) => id),
  widths: {}, sort: "artist", descending: false,
};
const SORTS: readonly string[] = ["name", "time", "artist", "album", "albumArtist", "genre", "year", "rating", "plays", "lastPlayed", "dateAdded", "kind", "bitrate", "sampleRate", "size", "playlistOrder", "composer", "trackNumber", "discNumber", "skipCount"];
const IDS = new Set<string>(COLUMN_DEFAULTS.map(([id]) => id));
const record = (value: unknown): Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
export const columnWidth = (id: ColumnId, value?: number) => Math.min(800, Math.max(id === "enabled" ? 22 : 40, Number.isFinite(value) ? Math.round(value!) : COLUMN_DEFAULTS.find(([key]) => key === id)![1]));
const knownIds = (value: unknown) => Array.isArray(value) ? [...new Set(value.filter((id): id is ColumnId => typeof id === "string" && IDS.has(id)))] : [];

/** New fields append hidden; unknown IDs, duplicates and unsafe widths are repaired. */
export function normalizeLayout(value: unknown): ColumnLayout {
  const raw = record(value), order = knownIds(raw.order), visible = raw.visible === undefined ? [...DEFAULT_LAYOUT.visible] : knownIds(raw.visible);
  if (!visible.includes("name")) visible.unshift("name");
  const widths: ColumnLayout["widths"] = {};
  for (const [id, width] of Object.entries(record(raw.widths))) {
    if (IDS.has(id) && typeof width === "number" && Number.isFinite(width)) widths[id as ColumnId] = columnWidth(id as ColumnId, width);
  }
  return { order: [...order, ...DEFAULT_LAYOUT.order.filter(id => !order.includes(id))], visible, widths,
    sort: typeof raw.sort === "string" && SORTS.includes(raw.sort) ? raw.sort as SortKey : DEFAULT_LAYOUT.sort, descending: raw.descending === true };
}

export function normalizeLibraryViews(value: unknown): LibraryViewPrefs {
  const raw = record(value), data = raw.version === 1 ? raw : {}, playlists: Record<string, ColumnLayout> = {};
  for (const [id, layout] of Object.entries(record(data.playlists))) if (/^\d+$/.test(id)) playlists[id] = normalizeLayout(layout);
  const browsing = record(data.browsing), customPresets: LibraryViewPrefs["customPresets"] = [];
  for (const entry of Array.isArray(data.customPresets) ? data.customPresets : []) {
    const preset = record(entry);
    if (typeof preset.name !== "string" || !preset.name.trim()) continue;
    const name = preset.name.trim().slice(0, 80);
    if (!customPresets.some(item => item.name === name)) customPresets.push({ name, layout: normalizeLayout(preset.layout) });
  }
  return { version: 1, libraryDefault: normalizeLayout(data.libraryDefault), playlists, customPresets,
    browsing: { browserVisible: browsing.browserVisible === true,
      viewMode: ["list", "albumList", "grid", "coverFlow"].includes(String(browsing.viewMode)) ? browsing.viewMode as BrowsingPreferences["viewMode"] : "list",
      gridGroup: ["albums", "artists", "genres", "composers"].includes(String(browsing.gridGroup)) ? browsing.gridGroup as BrowsingPreferences["gridGroup"] : "albums" } };
}
export function loadLibraryViews(): LibraryViewPrefs {
  try { return normalizeLibraryViews(JSON.parse(localStorage.getItem(LIBRARY_VIEWS_KEY) ?? "null")); }
  catch { return normalizeLibraryViews(null); }
}
export function saveLibraryViews(prefs: LibraryViewPrefs): void {
  try { localStorage.setItem(LIBRARY_VIEWS_KEY, JSON.stringify(prefs)); }
  catch { /* Session use still works when storage is unavailable. */ }
}
export const isLayoutSource = (source: string) => ["music", "audiobooks", "podcasts"].includes(source) || /^playlist:\d+$/.test(source);
export const playlistLayoutId = (source: string) => /^playlist:\d+$/.test(source) ? source.slice(9) : null;
export function layoutFor(prefs: LibraryViewPrefs, source: string, regular = false): ColumnLayout {
  const id = playlistLayoutId(source), saved = id !== null ? prefs.playlists[id] : undefined, layout = saved ?? prefs.libraryDefault;
  if (!saved && regular) return { ...layout, sort: "playlistOrder", descending: false };
  return !regular && layout.sort === "playlistOrder" ? { ...layout, sort: "artist", descending: false } : layout;
}
export function changeLayout(prefs: LibraryViewPrefs, source: string, changes: Partial<ColumnLayout>, regular = false): LibraryViewPrefs {
  if (!isLayoutSource(source)) return prefs;
  const id = playlistLayoutId(source), layout = normalizeLayout({ ...layoutFor(prefs, source, regular), ...changes });
  return id === null ? { ...prefs, libraryDefault: layout } : { ...prefs, playlists: { ...prefs.playlists, [id]: layout } };
}
export function restoreLayout(prefs: LibraryViewPrefs, source: string): LibraryViewPrefs {
  if (!isLayoutSource(source)) return prefs;
  const id = playlistLayoutId(source);
  if (id === null) return { ...prefs, libraryDefault: normalizeLayout(DEFAULT_LAYOUT) };
  const playlists = { ...prefs.playlists }; delete playlists[id];
  return { ...prefs, playlists };
}
export function copyLayoutToPlaylists(prefs: LibraryViewPrefs, layout: ColumnLayout, ids: number[]): LibraryViewPrefs {
  const playlists = { ...prefs.playlists };
  for (const id of new Set(ids)) if (Number.isSafeInteger(id) && id >= 0) playlists[String(id)] = normalizeLayout(layout);
  return { ...prefs, playlists };
}
const preset = (visible: ColumnId[], sort: SortKey): ColumnLayout => normalizeLayout({ ...DEFAULT_LAYOUT, order: visible, visible, sort });
export const BUILT_IN_PRESETS: { name: string; layout: ColumnLayout }[] = [
  { name: "Listening", layout: normalizeLayout(DEFAULT_LAYOUT) },
  { name: "Library Cleanup", layout: preset(["name", "artist", "album", "albumArtist", "composer", "year", "trackNumber", "discNumber", "genre"], "albumArtist") },
  { name: "Audio Quality", layout: preset(["name", "artist", "album", "kind", "bitrate", "sampleRate", "size", "time"], "bitrate") },
];
