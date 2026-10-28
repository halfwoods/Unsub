import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import {
  allowedColumn, BUILTIN_LAYOUTS, COLUMN_SPECS, defaultLayout, defaultViewPrefs, layoutForSource,
  moveColumn, normalizeLayout, parseViewPrefs, restoreSourceLayout, saveViewPrefs, VIEW_PREFS_KEY, withSourceLayout,
} from "../../preferences/libraryViews.ts";
import { CLEAR_BROWSE, hasBrowseFilters, selectBrowserFilter } from "../browseScope.ts";
import { formatMetadataDate, formatMetadataNumber, formatBitRate, formatSampleRate } from "../../util/format.ts";

const stored = new Map();
globalThis.localStorage = { getItem: (key) => stored.get(key) ?? null, setItem: (key, value) => stored.set(key, value) };
const storeUrl = new URL("../../state/store.ts", import.meta.url).href;
const calls = [];
let backend = {};
const apiModule = ["libraryTracks", "libraryStats", "libraryAlbums", "libraryArtists", "libraryArtistAlbums", "libraryGenres", "libraryComposers", "libraryBrowserFacets", "libraryRemoveFromPlaylist", "libraryPlaylists", "playerPlayList", "playerSetShuffle", "remoteTracks", "deviceTracks", "deviceStats", "cdTracks"].map((name) => `export const ${name} = (...args) => globalThis.__libraryViewApi('${name}', args);`).join("\n");
globalThis.__libraryViewApi = (name, args) => { calls.push([name, ...args]); return backend[name]?.(...args) ?? Promise.resolve(name === "libraryStats" ? { count: 0, totalDurationMs: 0, totalBytes: 0 } : []); };
registerHooks({ resolve(specifier, context, nextResolve) {
  if (context.parentURL === storeUrl && specifier === "../api/commands") return { url: `data:text/javascript,${encodeURIComponent(apiModule)}`, shortCircuit: true };
  if (context.parentURL === storeUrl && specifier === "../preferences/playbackPrefs") return { url: "data:text/javascript,export const loadVisualizerPrefs=()=>({});export const saveVisualizerPrefs=()=>{};", shortCircuit: true };
  if (context.parentURL === storeUrl && specifier === "../preferences/textSizes") return { url: "data:text/javascript,export const loadTextSizes=()=>({});export const listRowHeight=()=>18;export const saveTextSizes=()=>{};export const applyTextSizes=()=>{};", shortCircuit: true };
  return nextResolve(specifier, context);
} });
const { useApp } = await import(storeUrl);
const wait = () => new Promise((resolve) => setImmediate(resolve));
beforeEach(() => {
  calls.length = 0; backend = {}; stored.clear();
  useApp.setState({ source: "music", query: { kind: "music", search: null, sort: "artist", descending: false }, viewPrefs: defaultViewPrefs(), tracks: [], stats: null,
    viewMode: "list", gridGroup: "albums", browseHistory: [], browseLabel: null, browserFacets: null, loading: false,
    playlists: [{ id: 7, kind: "regular", name: "Seven" }, { id: 8, kind: "smart", name: "Eight" }, { id: 9, kind: "folder", name: "Folder" }], selection: new Set(), selectionAnchor: null, showNotice: () => {} });
});

test("layout round-trip preserves order, hidden fields, widths, sort and per-playlist overrides", () => {
  const layout = { ...moveColumn(defaultLayout(), "year", "name"), visible: ["name", "year", "albumArtist"], widths: { year: 105 }, sort: "year", descending: true };
  const prefs = withSourceLayout(defaultViewPrefs(), "playlist:7", layout);
  prefs.presets["My cleanup"] = layout;
  prefs.browser["playlist:7"] = true; prefs.mode = "grid"; prefs.group = "artists"; prefs.gridSize = 190;
  assert.equal(saveViewPrefs(prefs), true);
  const reopened = parseViewPrefs(stored.get(VIEW_PREFS_KEY));
  assert.deepEqual(reopened.overrides["playlist:7"], normalizeLayout(layout));
  assert.deepEqual(reopened.presets["My cleanup"], normalizeLayout(layout));
  assert.equal(reopened.browser["playlist:7"], true);
  assert.equal(reopened.group, "artists"); assert.equal(reopened.gridSize, 190);
  assert.equal(reopened.library.sort, "artist");
});

test("preference migration safely handles corrupt input, future versions, unknown IDs and widths", () => {
  for (const raw of [null, "null", "{bad", "[]", '{"version":200}']) assert.deepEqual(parseViewPrefs(raw), defaultViewPrefs());
  const value = normalizeLayout({ order: ["year", "year", "BPM"], visible: ["Comments", "year"], widths: { year: 99999, artist: -30, name: "bad", size: NaN }, sort: "DROP TABLE", descending: "yes" });
  assert.equal(new Set(value.order).size, COLUMN_SPECS.length);
  assert.deepEqual(value.visible, ["name", "year"]);
  assert.equal(value.widths.year, 900); assert.equal(value.widths.artist, 28); assert.equal(value.widths.name, undefined);
  assert.equal(value.sort, "artist"); assert.equal(value.descending, false);
});

test("playlist inheritance, explicit overrides and source restoration never overwrite other layouts", () => {
  let prefs = withSourceLayout(defaultViewPrefs(), "music", BUILTIN_LAYOUTS["Library Cleanup"]);
  assert.equal(layoutForSource(prefs, "playlist:7", true).sort, "playlistOrder");
  assert.deepEqual(layoutForSource(prefs, "playlist:7", true).visible, BUILTIN_LAYOUTS["Library Cleanup"].visible);
  prefs = withSourceLayout(prefs, "playlist:7", BUILTIN_LAYOUTS["Audio Quality"]);
  prefs = withSourceLayout(prefs, "device:test:music", BUILTIN_LAYOUTS.Listening);
  assert.equal(layoutForSource(prefs, "playlist:7", true).sort, "bitrate");
  assert.equal(prefs.library.sort, "albumArtist");
  assert.equal(restoreSourceLayout(prefs, "playlist:7").overrides["playlist:7"], undefined);
  assert.deepEqual(restoreSourceLayout(prefs, "music").library, defaultLayout());
  for (const source of ["device:test:music", "shared:test"]) {
    assert.equal(allowedColumn("rating", source), false); assert.equal(allowedColumn("enabled", source), false); assert.equal(allowedColumn("year", source), true);
  }
});

test("numeric and audio formatting is consistent and unavailable metadata stays empty", () => {
  assert.equal(formatMetadataDate(null), ""); assert.equal(formatMetadataDate(0), "");
  assert.equal(formatMetadataNumber(null), ""); assert.equal(formatMetadataNumber(0), "0");
  assert.equal(formatBitRate(null), ""); assert.equal(formatBitRate(320), "320 kbps");
  assert.equal(formatSampleRate(44100), "44.1 kHz");
});

test("progressive browser filters clear descendants while preserving search, playlist and IDs", () => {
  const query = { kind: "music", search: "live", playlist: 7, ids: [1, 2], sort: "playlistOrder", descending: false, genre: "Rock", artistGroup: "ann", albumGroup: "ann\x1fone" };
  const genre = selectBrowserFilter(query, "genre", "Pop");
  assert.equal(genre.artistGroup, null); assert.equal(genre.albumGroup, null);
  assert.equal(genre.playlist, 7); assert.equal(genre.search, "live"); assert.deepEqual(genre.ids, [1, 2]);
  assert.equal(selectBrowserFilter(query, "artistGroup", "bob").albumGroup, null);
  assert.equal(hasBrowseFilters({ ...query, ...CLEAR_BROWSE }), false);
});

test("artist and album navigation uses exact keys and Back restores scope without replacing search", async () => {
  useApp.setState({ source: "playlist:7", viewMode: "grid", gridGroup: "artists", query: { kind: null, playlist: 7, search: "live", ids: [1, 2], sort: "artist", descending: false } });
  useApp.getState().openBrowse("Ann", { artistGroup: "ann", albumGroup: null });
  await wait();
  assert.equal(useApp.getState().query.search, "live");
  assert.equal(calls.find(([name]) => name === "libraryArtistAlbums")[1], "ann");
  useApp.getState().openBrowse("First", { albumGroup: "ann\x1ffirst" }, true);
  await wait();
  assert.equal(useApp.getState().viewMode, "list");
  assert.equal(useApp.getState().query.playlist, 7);
  useApp.getState().backBrowse(); await wait();
  assert.equal(useApp.getState().query.albumGroup, null);
  assert.equal(useApp.getState().query.artistGroup, "ann");
  assert.equal(useApp.getState().viewMode, "grid");
  useApp.getState().backBrowse(); await wait();
  assert.equal(useApp.getState().gridGroup, "artists"); assert.equal(useApp.getState().query.artistGroup, null);
  assert.equal(useApp.getState().query.search, "live"); assert.deepEqual(useApp.getState().query.ids, [1, 2]);
});

test("width and visibility changes retain the in-flight query; sorts persist without affecting library defaults", async () => {
  useApp.getState().setSource("playlist:7"); await wait();
  const original = useApp.getState().query;
  const layout = layoutForSource(useApp.getState().viewPrefs, "playlist:7", true);
  useApp.getState().setColumnLayout({ ...layout, widths: { year: 130 }, visible: [...layout.visible, "year"] });
  assert.equal(useApp.getState().query, original);
  useApp.getState().sortBy("year"); await wait();
  assert.equal(useApp.getState().query.sort, "year");
  const persisted = parseViewPrefs(stored.get(VIEW_PREFS_KEY));
  assert.equal(persisted.overrides["playlist:7"].sort, "year"); assert.equal(persisted.library.sort, "artist");
  useApp.getState().setSource("music"); await wait(); assert.equal(useApp.getState().query.sort, "artist");
  useApp.getState().setSource("playlist:7"); await wait(); assert.equal(useApp.getState().query.sort, "year");
});

test("only explicitly selected playlists receive a layout; custom presets survive round-trip", () => {
  useApp.getState().setColumnLayout(BUILTIN_LAYOUTS["Audio Quality"]);
  useApp.getState().saveColumnPreset("Quality review");
  useApp.getState().applyLayoutToPlaylists([7, 9]);
  const prefs = parseViewPrefs(stored.get(VIEW_PREFS_KEY));
  assert.equal(prefs.overrides["playlist:7"].sort, "bitrate");
  assert.equal(prefs.overrides["playlist:8"], undefined); assert.equal(prefs.overrides["playlist:9"], undefined);
  assert.equal(prefs.presets["Quality review"].sort, "bitrate");
});

test("stale refreshes cannot replace the current grid even when the base query object is unchanged", async () => {
  let finish;
  backend.libraryTracks = () => new Promise((resolve) => { finish = resolve; });
  const first = useApp.getState().refresh();
  backend.libraryTracks = () => Promise.resolve([{ id: 2, title: "Current" }]);
  await useApp.getState().refresh();
  finish([{ id: 1, title: "Stale" }]); await first;
  assert.equal(useApp.getState().tracks[0].id, 2);
});

test("deleting a filtered playlist resolves positions from its full stored rows", async () => {
  useApp.setState({ source: "playlist:7", query: { kind: null, search: "needle", ids: [2], sort: "name", descending: false, genre: "Rock", artistGroup: "ann", albumGroup: "ann\x1fone" }, selection: new Set([2]) });
  backend.libraryTracks = () => Promise.resolve([{ id: 1 }, { id: 2 }, { id: 1 }, { id: 2 }]);
  await useApp.getState().deleteSelection();
  const resolved = calls.find(([name]) => name === "libraryTracks")[1];
  assert.equal(resolved.search, null); assert.equal(resolved.genre, null); assert.equal(resolved.artistGroup, null); assert.equal(resolved.albumGroup, null); assert.equal(resolved.ids, null);
  assert.deepEqual(calls.find(([name]) => name === "libraryRemoveFromPlaylist"), ["libraryRemoveFromPlaylist", 7, [1, 3]]);
});

test("hiding Column Browser clears browsing filters but keeps the playlist, search and columns", async () => {
  useApp.setState({ source: "playlist:7", query: { kind: null, playlist: 7, search: "live", genre: "Rock", artistGroup: "ann", sort: "playlistOrder", descending: false } });
  const layout = useApp.getState().viewPrefs.library;
  useApp.getState().setBrowserVisible(true); await wait();
  assert.ok(calls.some(([name]) => name === "libraryBrowserFacets"));
  useApp.getState().setBrowserVisible(false); await wait();
  const state = useApp.getState();
  assert.equal(hasBrowseFilters(state.query), false); assert.equal(state.query.search, "live"); assert.equal(state.query.playlist, 7);
  assert.equal(state.viewPrefs.library, layout); assert.equal(state.viewPrefs.browser["playlist:7"], false);
});

test("album playback preserves scoped duplicate entries and skips unchecked or unrelated songs", async () => {
  const key = "ann\x1ffirst";
  useApp.setState({ tracks: [
    { id: 2, albumGroup: key, enabled: true, kind: "music" },
    { id: 3, albumGroup: "anna\x1ffirst", enabled: true, kind: "music" },
    { id: 2, albumGroup: key, enabled: true, kind: "music" },
    { id: 4, albumGroup: key, enabled: false, kind: "music" },
  ] });
  await useApp.getState().playAlbum(key);
  assert.deepEqual(calls.find(([name]) => name === "playerPlayList"), ["playerPlayList", { type: "library" }, [2, 2], 0]);
});

test("album-list sorting stays grouped and its persisted mode agrees when a column preset changes it", async () => {
  useApp.getState().setColumnLayout(BUILTIN_LAYOUTS["Audio Quality"]); await wait();
  useApp.getState().setViewMode("albumList"); await wait();
  assert.equal(useApp.getState().query.sort, "album");
  assert.equal(parseViewPrefs(stored.get(VIEW_PREFS_KEY)).library.sort, "album");
  useApp.getState().setColumnLayout(BUILTIN_LAYOUTS["Audio Quality"]); await wait();
  assert.equal(useApp.getState().viewMode, "list");
  assert.equal(parseViewPrefs(stored.get(VIEW_PREFS_KEY)).mode, "list");
});

test("playlist-order presets map to artist order when applied outside a regular playlist", async () => {
  const layout = { ...defaultLayout(), sort: "playlistOrder", descending: true };
  useApp.getState().setColumnLayout(layout); await wait();
  assert.equal(useApp.getState().query.sort, "artist");
  useApp.setState({ source: "playlist:7", query: { kind: null, playlist: 7, search: null, sort: "playlistOrder", descending: false } });
  useApp.getState().applyLayoutToPlaylists([8]);
  assert.equal(useApp.getState().viewPrefs.overrides["playlist:8"].sort, "artist");
});
