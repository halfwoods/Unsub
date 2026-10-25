import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { BUILT_IN_PRESETS, LIBRARY_VIEWS_KEY, changeLayout, copyLayoutToPlaylists, columnWidth, layoutFor, loadLibraryViews, normalizeLibraryViews, normalizeLayout, restoreLayout, saveLibraryViews } from "../../preferences/libraryViews.ts";
import { formatColumnText, formatMetadataDate } from "../columnFormat.ts";

let storage;
beforeEach(() => { storage = new Map(); globalThis.localStorage = { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) }; });
test("column visibility, order, widths, sort and browsing survive a fresh read", () => {
  let prefs = normalizeLibraryViews(null);
  prefs = changeLayout(prefs, "music", { visible: ["name", "year", "sampleRate"], order: ["year", "name", "sampleRate"], widths: { name: 310, year: 80 }, sort: "year", descending: true });
  prefs = { ...prefs, browsing: { browserVisible: true, viewMode: "grid", gridGroup: "artists" } };
  saveLibraryViews(prefs); assert.deepEqual(loadLibraryViews(), prefs); assert.equal(layoutFor(loadLibraryViews(), "music").widths.name, 310);
});
test("corrupt, incompatible and unavailable storage recover safely", () => {
  storage.set(LIBRARY_VIEWS_KEY, "not JSON"); assert.equal(loadLibraryViews().libraryDefault.sort, "artist");
  storage.set(LIBRARY_VIEWS_KEY, JSON.stringify({ version: 999, libraryDefault: { sort: "size" } })); assert.equal(loadLibraryViews().libraryDefault.sort, "artist");
  globalThis.localStorage = { getItem() { throw Error("denied"); }, setItem() { throw Error("denied"); } };
  assert.doesNotThrow(() => saveLibraryViews(loadLibraryViews()));
});
test("normalization repairs malformed IDs, missing Name, dimensions and sort", () => {
  const layout = normalizeLayout({ order: ["year", "year", "future"], visible: ["future", "year"], widths: { name: -10, year: 99999, album: "100", future: 99 }, sort: "future" });
  assert.deepEqual(layout.order.slice(0, 2), ["year", "enabled"]); assert.equal(new Set(layout.order).size, layout.order.length);
  assert.deepEqual(layout.visible, ["name", "year"]); assert.equal(layout.widths.name, 40); assert.equal(layout.widths.year, 800);
  assert.equal(layout.widths.album, undefined); assert.equal(layout.sort, "artist"); assert.equal(columnWidth("enabled", 0), 22);
});
test("library changes propagate only to playlists without overrides", () => {
  let prefs = normalizeLibraryViews(null); assert.equal(layoutFor(prefs, "playlist:1", true).sort, "playlistOrder");
  prefs = changeLayout(prefs, "playlist:1", { visible: ["name", "year"], sort: "year", descending: true }, true);
  prefs = changeLayout(prefs, "music", { visible: ["name", "composer"], sort: "composer" });
  assert.deepEqual(layoutFor(prefs, "playlist:1", true).visible, ["name", "year"]); assert.equal(layoutFor(prefs, "playlist:1", true).sort, "year");
  assert.deepEqual(layoutFor(prefs, "playlist:2", true).visible, ["name", "composer"]); assert.equal(layoutFor(prefs, "playlist:2", true).sort, "playlistOrder");
  assert.equal(layoutFor(prefs, "playlist:3", false).sort, "composer"); prefs = restoreLayout(prefs, "playlist:1");
  assert.deepEqual(layoutFor(prefs, "playlist:1", true).visible, ["name", "composer"]); assert.equal(layoutFor(prefs, "playlist:1", true).sort, "playlistOrder");
});
test("explicit layout copy makes independent selected playlist snapshots", () => {
  let prefs = copyLayoutToPlaylists(normalizeLibraryViews(null), BUILT_IN_PRESETS[2].layout, [1, 3, 3]);
  assert.deepEqual(Object.keys(prefs.playlists), ["1", "3"]); prefs = changeLayout(prefs, "playlist:1", { widths: { name: 400 } }, true);
  assert.equal(prefs.playlists[3].widths.name, undefined); assert.ok(prefs.playlists[3].visible.includes("sampleRate"));
  prefs = restoreLayout(prefs, "music"); assert.ok(prefs.playlists[3].visible.includes("sampleRate"));
});
test("playlist order cannot leak through copied layouts to a library or smart playlist", () => {
  let prefs = normalizeLibraryViews(null);
  prefs = copyLayoutToPlaylists(prefs, { ...prefs.libraryDefault, sort: "playlistOrder", descending: true }, [1]);
  assert.equal(layoutFor(prefs, "playlist:1", false).sort, "artist"); assert.equal(layoutFor(prefs, "playlist:1", true).sort, "playlistOrder");
  prefs = { ...prefs, libraryDefault: prefs.playlists[1] }; assert.equal(layoutFor(prefs, "music").sort, "artist");
});
test("CD, device and shared sources cannot modify library layouts", () => {
  const prefs = normalizeLibraryViews(null);
  for (const source of ["cd:disc", "device:ipod:music", "shared:friend"]) {
    assert.equal(changeLayout(prefs, source, { sort: "year" }), prefs); assert.equal(restoreLayout(prefs, source), prefs);
  }
});
test("custom preset data is restored without duplicate names or unsafe layouts", () => {
  const prefs = normalizeLibraryViews({ version: 1, customPresets: [{ name: " Mine ", layout: { visible: ["year"] } }, { name: "Mine", layout: {} }, { name: "", layout: {} }] });
  assert.equal(prefs.customPresets.length, 1); assert.equal(prefs.customPresets[0].name, "Mine"); assert.deepEqual(prefs.customPresets[0].layout.visible, ["name", "year"]);
});
test("metadata formats seconds, audio units, ratings and missing values consistently", () => {
  const track = { title: "Test", year: null, trackNumber: null, discNumber: 2, bitrateKbps: 320, sampleRate: 44100, fileSize: 1048576, dateAdded: 1700000000, lastPlayed: null, playCount: 1234, skipCount: 0, codec: "flac", durationMs: 65000, rating: 20 };
  assert.equal(formatColumnText("year", track), ""); assert.equal(formatColumnText("trackNumber", track), ""); assert.equal(formatColumnText("lastPlayed", track), "");
  assert.equal(formatColumnText("dateAdded", track), formatMetadataDate(1700000000)); assert.match(formatColumnText("bitrate", track), /320 kbps/);
  assert.match(formatColumnText("sampleRate", track), /44[.,]1 kHz/); assert.equal(formatColumnText("size", track), "1 MB");
  assert.equal(formatColumnText("kind", track), "FLAC audio"); assert.equal(formatColumnText("time", track), "1:05");
  assert.equal(formatColumnText("plays", track), new Intl.NumberFormat().format(1234)); assert.equal(formatMetadataDate(0), ""); assert.equal(formatMetadataDate(NaN), "");
  assert.equal(formatColumnText("rating", track), "★"); assert.equal(formatColumnText("rating", { ...track, rating: 80 }), "★★★★");
});
