import assert from "node:assert/strict";
import { test } from "node:test";
import { albumBlocks, albumRowWindow, orderAlbumTracks, shownTracksOf, summaryAlbumKey, trackAlbumKey } from "../albumKeys.ts";

test("canonical album identities honor normalized tags and compilations without merging same titles", () => {
  const a = { id: 1, album: "Live", albumArtist: "Beyoncé", albumGroup: "beyonce\u001flive", kind: "music" };
  const b = { id: 2, album: "Live", albumArtist: "Other", albumGroup: "other\u001flive", kind: "music" };
  const c = { id: 3, album: "Mix", artist: "One", compilation: true, albumGroup: "\u001fmix", kind: "music" };
  const d = { ...c, id: 4, artist: "Two" };
  const rows = orderAlbumTracks([a, b, c, a, d]);
  assert.deepEqual(rows.map(t => t.id), [1, 1, 2, 3, 4]);
  assert.deepEqual(albumBlocks(rows).map(g => [g.key, g.start, g.tracks.length]), [[a.albumGroup, 0, 2], [b.albumGroup, 2, 1], [c.albumGroup, 3, 2]]);
  const summary = { key: a.albumGroup, album: "LIVE", albumArtist: "BEYONCE" };
  assert.equal(trackAlbumKey(a), summaryAlbumKey(summary));
  assert.deepEqual(shownTracksOf(rows, summary), [a, a]);
  assert.deepEqual(albumBlocks([]), []);
});

test("a giant album renders only viewport rows with overscan at every scroll position", () => {
  assert.deepEqual(albumRowWindow(12000, 20, 20, 600, 20), {start:0,end:35});
  assert.deepEqual(albumRowWindow(12000, 20, 120020, 600, 20), {start:5995,end:6035});
  assert.deepEqual(albumRowWindow(12000, 20, 240000, 600, 20), {start:11994,end:12000});
});
