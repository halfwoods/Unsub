import assert from "node:assert/strict";
import { test } from "node:test";
import { artistInitials, browseScrollKey, nextGridIndex } from "../browseHelpers.ts";
const query = { kind: "music", search: null, sort: "artist", descending: false };
test("scroll positions belong to exact paths inside their surrounding scope", () => {
  const base = browseScrollKey("music", "albums", { ...query, artistKey: "air" });
  for (const changed of [{ artistKey: "air supply" }, { artistKey: "air", albumKey: "air\u001fmoon safari" }, { artistKey: "air", search: "live" },
    { artistKey: "air", playlist: 7 }, { artistKey: "air", kind: "audiobook" }, { artistKey: "air", genre: "Electronic" }, { artistKey: "air", composer: "Dunckel" }, { artistKey: "air", ids: [1, 2] }]) {
    assert.notEqual(base, browseScrollKey("music", "albums", { ...query, ...changed }));
  }
  assert.notEqual(base, browseScrollKey("playlist:7", "albums", { ...query, artistKey: "air" }));
  assert.notEqual(base, browseScrollKey("music", "artists", { ...query, artistKey: "air" }));
});
test("keyboard grid movement remains bounded in an incomplete last row", () => {
  assert.equal(nextGridIndex(4, "ArrowDown", 3, 7), 6); assert.equal(nextGridIndex(6, "ArrowRight", 3, 7), 6);
  assert.equal(nextGridIndex(0, "ArrowLeft", 3, 7), 0); assert.equal(nextGridIndex(5, "ArrowUp", 3, 7), 2);
  assert.equal(nextGridIndex(3, "Home", 3, 7), 0); assert.equal(nextGridIndex(3, "End", 3, 7), 6); assert.equal(nextGridIndex(0, "End", 1, 0), 0);
});
test("artist fallback initials handle blank and long names", () => {
  assert.equal(artistInitials(""), "?"); assert.equal(artistInitials("  Compilations  "), "C");
  assert.equal(artistInitials("Nina Simone"), "NS"); assert.equal(artistInitials("  A very long artist name  "), "AN");
});
