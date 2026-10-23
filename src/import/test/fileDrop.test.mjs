import assert from "node:assert/strict";
import { test } from "node:test";

import { importDropped, watchFileDrops } from "../fileDrop.ts";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function imports(overrides = {}) {
  const calls = [];
  return {
    calls,
    actions: {
      addMusic: async (paths) => (calls.push(["music", paths]), true),
      musicAdded: async () => calls.push(["welcomeDone"]),
      addStations: async (path) => (calls.push(["stations", path]), [1, 2]),
      stationsAdded: (count) => calls.push(["stationCount", count]),
      report: (error) => calls.push(["error", String(error)]),
      ...overrides,
    },
  };
}

test("a dropped music file and folder go together to the library before completing the welcome", async () => {
  let accept;
  const { actions, calls } = imports({
    addMusic: (paths) => (calls.push(["music", paths]), new Promise((resolve) => { accept = resolve; })),
  });
  const done = importDropped(["/Albums/One.flac", "/My music"], actions);
  assert.deepEqual(calls, [["music", ["/Albums/One.flac", "/My music"]]]);
  accept(true);
  await done;
  assert.deepEqual(calls.at(-1), ["welcomeDone"]);
});

test("a rejected music import leaves the welcome available to retry", async () => {
  const { actions, calls } = imports({ addMusic: async () => false });
  await importDropped(["/inaccessible/music"], actions);
  assert.deepEqual(calls, []);
});

test("unexpected import errors are shown rather than becoming unhandled rejections", async () => {
  const { actions, calls } = imports({ addMusic: async () => { throw "Permission denied"; } });
  await importDropped(["/Albums"], actions);
  assert.deepEqual(calls, [["error", "Permission denied"]]);
});

test("station lists keep their station routing and never dismiss the music welcome", async () => {
  const { actions, calls } = imports();
  await importDropped(["/Radio.PLS", "/More.m3u"], actions);
  assert.deepEqual(calls, [
    ["stations", "/Radio.PLS"],
    ["stations", "/More.m3u"],
    ["stationCount", 2],
  ]);
});

test("a broken station list does not stop music from a mixed drop being added", async () => {
  const { actions, calls } = imports({ addStations: async () => { throw "Broken station list"; } });
  await importDropped(["/Albums", "/Radio.pls"], actions);
  assert.ok(calls.some(([name]) => name === "welcomeDone"));
  assert.ok(calls.some(([name, value]) => name === "error" && value === "Broken station list"));
});

test("empty drops do nothing", async () => {
  const { actions, calls } = imports();
  await importDropped([], actions);
  assert.deepEqual(calls, []);
});

test("native enter, cancel and drop update the visible target and add paths only on drop", async () => {
  let emit;
  let stopped = 0;
  const hovering = [];
  const dropped = [];
  const stop = watchFileDrops(
    async (handler) => (emit = handler, () => stopped++),
    (active) => hovering.push(active),
    async (paths) => dropped.push(paths),
    assert.fail,
  );
  await settle();
  emit({ type: "enter", paths: ["/Music"] });
  emit({ type: "over" });
  assert.deepEqual(hovering, [true]);
  assert.deepEqual(dropped, []);
  emit({ type: "leave" });
  emit({ type: "enter", paths: ["/Music"] });
  emit({ type: "drop", paths: ["/Music"] });
  assert.deepEqual(hovering, [true, false, true, false]);
  assert.deepEqual(dropped, [["/Music"]]);
  stop();
  assert.equal(stopped, 1);
  emit({ type: "drop", paths: ["/after unmount"] });
  assert.equal(dropped.length, 1);
});

test("an obsolete async subscription cannot import twice during React StrictMode remounts", async () => {
  let emit;
  let ready;
  let stopped = 0;
  const stop = watchFileDrops(
    (handler) => (emit = handler, new Promise((resolve) => { ready = resolve; })),
    assert.fail,
    assert.fail,
    assert.fail,
  );
  stop();
  emit({ type: "drop", paths: ["/Music"] });
  ready(() => stopped++);
  await settle();
  assert.equal(stopped, 1);
});

test("native listener failures and rejected drop handlers are reported", async () => {
  const errors = [];
  watchFileDrops(async () => { throw "Listener unavailable"; }, assert.fail, assert.fail, (error) => errors.push(error));
  let emit;
  const stop = watchFileDrops(
    async (handler) => (emit = handler, () => {}),
    () => {},
    async () => { throw "Couldn't add files"; },
    (error) => errors.push(error),
  );
  emit({ type: "drop", paths: ["/Music"] });
  await settle();
  stop();
  assert.deepEqual(errors, ["Listener unavailable", "Couldn't add files"]);
});
