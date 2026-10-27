import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import { beforeEach, test } from "node:test";

const storeUrl = new URL("../../state/store.ts", import.meta.url).href;
const commandMock = `
  const state = () => globalThis.__browseTest;
  export const libraryTracks = async (query) => { state().queries.push(query); return state().read(query); };
  export const libraryTrack = async (id) => { state().trackLookups.push(id); return state().tracks.find(t => t.id === id) ?? null; };
  export const deviceTracks = async (id, view, query) => { state().deviceQueries.push({id,view,query}); return state().deviceRead(query); };
  export const deviceStats = async () => ({ count: 1, totalDurationMs: 0, totalBytes: 0 });
  export const remoteTracks = async (id, query) => { state().remoteQueries.push({id,query}); return state().deviceRead(query); };
  export const cdTracks = async (id) => { state().cdQueries.push(id); return state().deviceRead({}); };
  export const libraryStats = async () => ({ count: state().tracks.length, totalDurationMs: 0, totalBytes: 0 });
  export const libraryAlbums = async (query) => { state().albumQueries.push(query); return []; };
  export const libraryArtistAlbums = async (key, query) => { state().artistAlbumQueries.push({key,query}); return []; };
  export const libraryArtists = async () => [];
  export const libraryGenres = async () => [];
  export const libraryComposers = async () => [];
  export const libraryAlbumTracks = async (key, query) => { state().albumTrackQueries.push({key,query}); return state().tracks; };
  export const playerPlayList = async (source, ids, index) => { state().queues.push({source,ids,index}); };
  export const playerSetShuffle = async () => {};
  export const libraryRemoveFromPlaylist = async (id, positions) => { state().removed.push({id,positions}); };
  export const libraryPlaylists = async () => state().playlists;
`;
registerHooks({ resolve(specifier, context, next) {
  if (context.parentURL === storeUrl && specifier === "../api/commands") return { url: `data:text/javascript,${encodeURIComponent(commandMock)}`, shortCircuit: true };
  if (specifier.startsWith(".") && context.parentURL?.startsWith("file:")) {
    const url = new URL(specifier, context.parentURL);
    if (!/\.[a-z]+$/i.test(url.pathname) && existsSync(new URL(`${url.href}.ts`))) return { url: `${url.href}.ts`, shortCircuit: true };
  }
  return next(specifier, context);
} });
const storage = new Map();
globalThis.localStorage = { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value), removeItem: (key) => storage.delete(key) };
const { useApp } = await import(storeUrl);
const { useLibraryViews } = await import("../../preferences/libraryViewStore.ts");
const initial = useApp.getState();
const initialPrefs = useLibraryViews.getState().prefs;
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(async () => {
  const tracks = [{ id: 1, enabled: true, kind: "music", missing: false }, { id: 1, enabled: true, kind: "music", missing: false }, { id: 2, enabled: false, kind: "music", missing: false }];
  globalThis.__browseTest = { tracks, queries: [], albumQueries: [], artistAlbumQueries: [], albumTrackQueries: [], queues: [], removed: [],
    trackLookups: [], deviceQueries: [], remoteQueries: [], cdQueries: [], notices: [], deviceRead: async () => [],
    playlists: [{ id: 42, name: "Mix", kind: "regular" }], read: async () => tracks };
  useApp.setState(initial, true);
  useLibraryViews.setState({ prefs: initialPrefs });
  useApp.setState({ playlists: globalThis.__browseTest.playlists, showNotice: message => globalThis.__browseTest.notices.push(message) });
  await settle();
});

test("artist → album → back preserves playlist/search and exact grouping keys", async () => {
  useApp.getState().setSource("playlist:42");
  useApp.getState().setSearch("live");
  useApp.getState().openArtist("ann", "Ann");
  await settle();
  assert.deepEqual(globalThis.__browseTest.artistAlbumQueries.at(-1).key, "ann");
  useApp.getState().openAlbum("ann\u001flive", "Live");
  await settle();
  const state = useApp.getState();
  assert.equal(state.query.search, "live"); assert.equal(state.query.playlist, 42);
  assert.equal(state.query.artistKey, "ann"); assert.equal(state.query.albumKey, "ann\u001flive"); assert.equal(state.viewMode, "list");
  state.setBrowseScroll("artist-root", 850);
  state.browseBack(); await settle();
  assert.equal(useApp.getState().query.albumKey, null); assert.equal(useApp.getState().query.artistKey, "ann");
  useApp.getState().browseBack(); await settle();
  assert.equal(useApp.getState().gridGroup, "artists"); assert.equal(useApp.getState().query.artistKey, null);
  assert.equal(useApp.getState().query.search, "live"); assert.equal(useApp.getState().browseScroll["artist-root"], 850);
});

test("scoped playback keeps duplicate entries and skips unchecked songs", async () => {
  useApp.setState({ tracks: globalThis.__browseTest.tracks });
  await useApp.getState().playBrowse(false);
  assert.deepEqual(globalThis.__browseTest.queues.at(-1).ids, [1, 1]);
  useApp.getState().setSearch("live"); useApp.getState().openArtist("ann", "Ann");
  await settle(); await useApp.getState().playAlbum("ann\u001flive");
  const call = globalThis.__browseTest.albumTrackQueries.at(-1);
  assert.equal(call.query.search, "live"); assert.equal(call.query.artistKey, "ann");
  assert.deepEqual(globalThis.__browseTest.queues.at(-1).ids, [1, 1]);
});

test("Column Browser choices narrow progressively, clear and never leak to devices", async () => {
  useApp.getState().setBrowserFilter("genre", "Jazz");
  useApp.getState().setBrowserFilter("artistKey", "ann");
  useApp.getState().setBrowserFilter("albumKey", "one");
  await settle(); assert.equal(useApp.getState().query.genre, "Jazz");
  useApp.getState().setBrowserFilter("genre", "Rock"); await settle();
  assert.equal(useApp.getState().query.artistKey, null); assert.equal(useApp.getState().query.albumKey, null);
  useApp.getState().clearBrowser(); await settle(); assert.equal(useApp.getState().query.genre, null);
  useApp.getState().openArtist("ann", "Ann"); await settle();
  useApp.getState().setSource("device:test");
  assert.equal(useApp.getState().query.artistKey, null); assert.deepEqual(useApp.getState().browsePath, []);
});

test("filtered playlist deletion resolves positions from the complete stored playlist", async () => {
  useApp.getState().setSource("playlist:42"); useApp.getState().setSearch("live");
  useApp.getState().setBrowserFilter("genre", "Jazz"); await settle();
  useApp.setState({ selection: new Set([1]), viewMode: "list" });
  await useApp.getState().deleteSelection();
  const query = globalThis.__browseTest.queries.findLast((q) => q.sort === "playlistOrder" && q.search === null && q.genre === null);
  assert.ok(query); assert.equal(query.ids, null); assert.equal(query.playlist, 42);
  assert.deepEqual(globalThis.__browseTest.removed, [{ id: 42, positions: [0, 1] }]);
});

test("late responses cannot overwrite a newer search", async () => {
  let resolveSlow;
  globalThis.__browseTest.read = (query) => query.search === "slow" ? new Promise((resolve) => { resolveSlow = resolve; }) : Promise.resolve([{ id: 9 }]);
  useApp.getState().setSearch("slow");
  useApp.getState().setSearch("fast"); await settle();
  resolveSlow([{ id: 8 }]); await settle();
  assert.equal(useApp.getState().query.search, "fast"); assert.deepEqual(useApp.getState().tracks, [{ id: 9 }]);
});

test("album drill-down restores the descending root sort and supports an empty artist key", async () => {
  useApp.getState().sortBy("year"); useApp.getState().sortBy("year"); await settle();
  useApp.getState().openArtist("", "Unknown Artist"); await settle();
  assert.equal(globalThis.__browseTest.artistAlbumQueries.at(-1).key, "");
  useApp.getState().openAlbum("\u001funtitled", "Untitled"); await settle();
  assert.equal(useApp.getState().query.sort, "album"); assert.equal(useApp.getState().query.descending, false);
  useApp.getState().browseBack(); await settle(); useApp.getState().browseBack(); await settle();
  assert.equal(useApp.getState().query.sort, "year"); assert.equal(useApp.getState().query.descending, true);
  assert.equal(useApp.getState().query.artistKey, null);
});

test("Album List keeps album grouping when changing sources or applying a preset", async () => {
  useLibraryViews.getState().updateLayout("music", { sort: "size", descending: true });
  useApp.getState().setViewMode("albumList"); await settle();
  assert.equal(useApp.getState().query.sort, "album");
  useApp.getState().setSource("playlist:42"); await settle();
  assert.equal(useApp.getState().query.sort, "album");
  useLibraryViews.getState().updateLayout("playlist:42", { sort: "bitrate", descending: true }, true); await settle();
  assert.equal(useApp.getState().query.sort, "album"); assert.equal(useApp.getState().query.descending, false);
  useApp.getState().showMusicView("songs"); await settle();
  assert.equal(useApp.getState().query.sort, "size"); assert.equal(useApp.getState().query.descending, true);
});

test("column-only edits do not reload tracks and sorting clears the positional selection anchor", async () => {
  useApp.setState({ selectionAnchor: 9 });
  const query = useApp.getState().query, reads = globalThis.__browseTest.queries.length;
  useLibraryViews.getState().updateLayout("music", { widths: { name: 310 }, visible: ["name", "year"] }); await settle();
  assert.equal(useApp.getState().query, query); assert.equal(globalThis.__browseTest.queries.length, reads);
  useApp.getState().sortBy("composer"); await settle();
  assert.equal(useApp.getState().selectionAnchor, null); assert.equal(useLibraryViews.getState().prefs.libraryDefault.sort, "composer");
});

test("pending scope changes disable old-scope playback and showing the browser persists Songs view", async () => {
  let complete;
  useApp.setState({ tracks: globalThis.__browseTest.tracks });
  globalThis.__browseTest.read = () => new Promise(resolve => { complete = resolve; });
  useApp.getState().setSearch("pending");
  await useApp.getState().playBrowse(); assert.equal(globalThis.__browseTest.queues.length, 0);
  complete([]); await settle();
  globalThis.__browseTest.read = async () => [];
  useApp.getState().setViewMode("grid"); useApp.getState().toggleColumnBrowser(); await settle();
  assert.equal(useApp.getState().viewMode, "list"); assert.equal(useLibraryViews.getState().prefs.browsing.viewMode, "list");
  assert.equal(useLibraryViews.getState().prefs.browsing.browserVisible, true);
  useApp.getState().setBrowserFilter("genre", "jazz"); await settle();
  useApp.getState().toggleColumnBrowser(); await settle();
  assert.equal(useApp.getState().query.genre, null);
});

function playing(id = 1, source = { type: "library" }, state = "playing") {
  useApp.setState({ playingSource: source, playingTracks: globalThis.__browseTest.tracks,
    playback: { ...initial.playback, trackId: id, state }, loadedQuery: useApp.getState().query });
}

test("Go to Current Song preserves a visible filtered playlist and reissues scrolling for the selected duplicate", async () => {
  useApp.getState().setSource("playlist:42"); useApp.getState().setSearch("live");
  useApp.getState().setBrowserFilter("genre", "jazz"); await settle(); playing();
  useApp.setState({ selectionAnchor: 1, selection: new Set([1, 2]) });
  const query = useApp.getState().query, playback = useApp.getState().playback;
  const reads = globalThis.__browseTest.queries.length;
  await useApp.getState().revealCurrentSong();
  const first = useApp.getState().songReveal;
  assert.equal(useApp.getState().query, query); assert.equal(useApp.getState().selectionAnchor, 1);
  assert.deepEqual([...useApp.getState().selection], [1]); assert.equal(first.trackId, 1);
  await useApp.getState().revealCurrentSong();
  assert.notEqual(useApp.getState().songReveal, first); assert.equal(useApp.getState().playback, playback);
  assert.equal(globalThis.__browseTest.queries.length, reads); assert.equal(globalThis.__browseTest.queues.length, 0);
});

test("Go to Current Song clears hiding filters while retaining playlist order and does not restart playback", async () => {
  useApp.getState().setSource("playlist:42"); await settle(); playing();
  useApp.setState({ query: { ...useApp.getState().query, search: "hidden", ids: [2], artistKey: "different", genre: "rock", composer: "other" },
    tracks: [], duplicates: { exact: true }, browserFilters: { genre: "rock", artistKey: "different", albumKey: null } });
  await useApp.getState().revealCurrentSong();
  const state = useApp.getState();
  assert.equal(state.source, "playlist:42"); assert.equal(state.query.playlist, 42); assert.equal(state.query.sort, "playlistOrder");
  for (const field of ["search", "ids", "artistKey", "genre", "composer"]) assert.equal(state.query[field], null);
  assert.equal(state.duplicates, null); assert.deepEqual([...state.selection], [1]); assert.equal(state.songReveal.trackId, 1);
  assert.equal(globalThis.__browseTest.queues.length, 0);
});

test("Go to Current Song falls back from an unrelated playlist to Music using the saved layout sort", async () => {
  useLibraryViews.getState().updateLayout("music", { sort: "year", descending: true });
  globalThis.__browseTest.read = async query => query.playlist ? [] : globalThis.__browseTest.tracks;
  useApp.getState().setSource("playlist:42"); await settle(); playing();
  await useApp.getState().revealCurrentSong();
  const state = useApp.getState();
  assert.equal(state.source, "music"); assert.equal(state.query.playlist, null);
  assert.equal(state.query.sort, "year"); assert.equal(state.query.descending, true);
  assert.equal(state.selectionAnchor, 0); assert.equal(state.songReveal.trackId, 1);
});

test("Go to Current Song opens Songs from a grid or visualizer and supports paused audiobooks", async () => {
  globalThis.__browseTest.tracks = [{ ...globalThis.__browseTest.tracks[0], kind: "audiobook" }];
  globalThis.__browseTest.read = async query => query.kind === "audiobook" ? globalThis.__browseTest.tracks : [];
  useApp.getState().setViewMode("grid"); await settle(); playing(1, { type: "library" }, "paused");
  useApp.setState({ visualizer: { ...initial.visualizer, open: true } });
  await useApp.getState().revealCurrentSong();
  const state = useApp.getState();
  assert.equal(state.source, "audiobooks"); assert.equal(state.viewMode, "list"); assert.equal(state.visualizer.open, false);
  assert.equal(state.playback.state, "paused"); assert.equal(state.query.kind, "audiobook");
  assert.equal(state.songReveal.trackId, 1);
});

test("Go to Current Song resolves device, CD and shared IDs in their own sources", async () => {
  for (const [source, destination, log] of [
    [{ type: "device", deviceId: "ipod" }, "device:ipod:music", "deviceQueries"],
    [{ type: "cd", discId: "disc" }, "cd:disc", "cdQueries"],
    [{ type: "shared", library: "server:test" }, "shared:server:test", "remoteQueries"],
  ]) {
    useApp.setState({ source: "music", query: { ...initial.query }, tracks: [{ id: 1, title: "Wrong local song" }],
      devices: [{ id: "ipod" }], cds: [{ id: "disc" }], shared: [{ id: "server:test", hasLogin: true }] });
    globalThis.__browseTest.deviceRead = async () => [{ id: 1, kind: "music", title: "External song" }]; playing(1, source);
    const libraryReads = globalThis.__browseTest.queries.length;
    await useApp.getState().revealCurrentSong();
    assert.equal(useApp.getState().source, destination); assert.equal(useApp.getState().tracks[0].title, "External song");
    assert.equal(useApp.getState().songReveal.source, destination); assert.ok(globalThis.__browseTest[log].length > 0);
    assert.equal(globalThis.__browseTest.queries.length, libraryReads); assert.equal(globalThis.__browseTest.trackLookups.length, 0);
  }
});

test("Go to Current Song no-ops when stopped or live, and reports disconnected or deleted songs", async () => {
  const source = useApp.getState().source, query = useApp.getState().query;
  await useApp.getState().revealCurrentSong(); assert.equal(useApp.getState().songReveal, null);
  playing(); useApp.setState({ playback: { ...useApp.getState().playback, live: true } });
  await useApp.getState().revealCurrentSong(); assert.equal(useApp.getState().songReveal, null);
  playing(1, { type: "device", deviceId: "missing" });
  await useApp.getState().revealCurrentSong(); assert.equal(globalThis.__browseTest.deviceQueries.length, 0);
  assert.equal(globalThis.__browseTest.notices.length, 1);
  playing(999); useApp.setState({ playingTracks: [] });
  await useApp.getState().revealCurrentSong(); assert.equal(globalThis.__browseTest.notices.length, 2);
  assert.equal(useApp.getState().source, source); assert.equal(useApp.getState().query, query); assert.equal(useApp.getState().songReveal, null);
});

test("Go to Current Song cannot steal selection after navigation or a playback change during lookup", async () => {
  for (const change of [() => useApp.getState().setSearch("new search"), () => playing(2)]) {
    let complete;
    playing(); useApp.setState({ tracks: [], loadedQuery: null, songReveal: null });
    globalThis.__browseTest.read = query => query.ids ? new Promise(resolve => { complete = resolve; }) : Promise.resolve([]);
    const pending = useApp.getState().revealCurrentSong(); change(); complete(globalThis.__browseTest.tracks);
    await pending; await settle();
    assert.equal(useApp.getState().songReveal, null); assert.equal(useApp.getState().selectionAnchor, null);
  }
});

test("Go to Current Song yields to a dialog opened while looking up a hidden song", async () => {
  let complete, dialogOpen = false;
  playing(); useApp.setState({ tracks: [], loadedQuery: null });
  const query = useApp.getState().query;
  globalThis.__browseTest.read = () => new Promise(resolve => { complete = resolve; });
  const pending = useApp.getState().revealCurrentSong(() => !dialogOpen);
  dialogOpen = true; complete(globalThis.__browseTest.tracks); await pending;
  assert.equal(useApp.getState().query, query); assert.equal(useApp.getState().songReveal, null);
  assert.equal(useApp.getState().selectionAnchor, null);
});

test("Album List makes canonical groups contiguous and playback retains duplicate positions", async () => {
  const a = { id: 11, albumGroup: "a", enabled: true, kind: "music", missing: false };
  const b = { ...a, id: 12, albumGroup: "b" };
  globalThis.__browseTest.read = async () => [a, b, a];
  useApp.getState().setViewMode("albumList"); await settle();
  assert.deepEqual(useApp.getState().tracks.map(t => t.id), [11, 11, 12]);
  await useApp.getState().playRow(1);
  assert.deepEqual(globalThis.__browseTest.queues.at(-1).ids, [11, 11, 12]);
  assert.equal(globalThis.__browseTest.queues.at(-1).index, 1);
});

test("Album List heading sorts do not replace saved Songs sorting", async () => {
  useApp.getState().sortBy("composer"); await settle();
  const saved = useLibraryViews.getState().prefs.libraryDefault;
  useApp.getState().setViewMode("albumList"); await settle();
  useApp.getState().sortBy("year"); await settle();
  assert.equal(useApp.getState().query.sort, "year");
  assert.deepEqual(useLibraryViews.getState().prefs.libraryDefault, saved);
  useApp.getState().setViewMode("list"); await settle();
  assert.equal(useApp.getState().query.sort, "composer");
  assert.equal(useApp.getState().query.descending, saved.descending);
});
