import assert from "node:assert/strict";
import { test } from "node:test";
import { browseQuery, browserQueries, EMPTY_BROWSER, narrowBrowser, playlistPositionQuery, hasBrowseFilters } from "../browseScope.ts";

const base = { kind: "music", search: "live", playlist: 42, ids: [1, 2, 3], sort: "year", descending: true };

test("exact navigation intersects playlist, search, kind and ID scope without replacing text search", () => {
  const result = browseQuery(base, { ...EMPTY_BROWSER, genre: "Rock" }, [{ kind: "artist", key: "beyonce", label: "Beyoncé" }, { kind: "album", key: "beyonce\u001flive", label: "Live" }]);
  assert.equal(result.search, "live"); assert.equal(result.playlist, 42); assert.equal(result.kind, "music");
  assert.deepEqual(result.ids, [1, 2, 3]); assert.equal(result.artistKey, "beyonce"); assert.equal(result.genre, "Rock");
  assert.equal(result.albumKey, "beyonce\u001flive"); assert.equal(result.sort, "year"); assert.ok(hasBrowseFilters(result));
});

test("changing upstream browser choices clears downstream keys only", () => {
  const filters = { genre: "Rock", artistKey: "ann", albumKey: "ann\u001fone" };
  assert.deepEqual(narrowBrowser(filters, "genre", "Jazz"), { genre: "Jazz", artistKey: null, albumKey: null });
  assert.deepEqual(narrowBrowser(filters, "artistKey", "bob"), { genre: "Rock", artistKey: "bob", albumKey: null });
  assert.deepEqual(narrowBrowser(filters, "albumKey", null), { ...filters, albumKey: null });
});

test("facets retain base scope while dropping self and downstream restrictions", () => {
  const query = { ...base, genre: "Jazz", artistKey: "ann", albumKey: "one", composer: "Ada" };
  const facets = browserQueries(query);
  for (const facet of Object.values(facets)) {
    assert.equal(facet.playlist, 42); assert.equal(facet.search, "live"); assert.equal(facet.kind, "music");
    assert.deepEqual(facet.ids, [1, 2, 3]); assert.equal(facet.composer, "Ada"); assert.equal(facet.albumKey, null);
  }
  assert.equal(facets.genres.genre, null); assert.equal(facets.genres.artistKey, null);
  assert.equal(facets.artists.genre, "Jazz"); assert.equal(facets.artists.artistKey, null);
  assert.equal(facets.albums.genre, "Jazz"); assert.equal(facets.albums.artistKey, "ann");
});

test("back removes only navigation and restores retained browser filters", () => {
  const filters = { ...EMPTY_BROWSER, genre: "Jazz" };
  const opened = browseQuery(base, filters, [{ kind: "artist", key: "compilations", label: "Compilations" }]);
  const back = browseQuery(opened, filters, []);
  assert.equal(back.artistKey, null); assert.equal(back.genre, "Jazz"); assert.equal(back.search, "live");
});

test("stored playlist positions never inherit display filters or duplicate-finder IDs", () => {
  const query = playlistPositionQuery({ ...base, genre: "Jazz", artistKey: "ann", albumKey: "one", composer: "Ada" });
  assert.equal(query.playlist, 42); assert.equal(query.search, null); assert.equal(query.kind, null); assert.equal(query.ids, null);
  assert.equal(query.sort, "playlistOrder"); assert.equal(query.descending, false); assert.equal(hasBrowseFilters(query), false);
});
