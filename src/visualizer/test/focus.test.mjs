import assert from "node:assert/strict";
import { test } from "node:test";
import { attachVisualizerFocus } from "../focus.ts";

const tick = () => new Promise((resolve) => setImmediate(resolve));

function fixture() {
  const events = [];
  let resize;
  const host = {
    onResize: async (callback) => { resize = callback; return () => events.push("unlisten"); },
    isWindowFocused: async () => true,
    canRestore: () => true,
    focusWebview: async () => { events.push("webview"); },
    focusContent: () => events.push("content"),
    onError: () => events.push("error"),
  };
  return { host, events, resize: () => resize() };
}

test("mount and fullscreen completion restore native webview before DOM focus", async () => {
  const fixture_ = fixture();
  const dispose = attachVisualizerFocus(fixture_.host);
  await tick();
  assert.deepEqual(fixture_.events, ["webview", "content"]);
  fixture_.resize(); // Tauri's resize notification after entering fullscreen.
  await tick();
  fixture_.resize(); // And after exiting fullscreen; the listener must still be attached.
  await tick();
  assert.deepEqual(fixture_.events, ["webview", "content", "webview", "content", "webview", "content"]);
  dispose();
  assert.equal(fixture_.events.at(-1), "unlisten");
});

test("an inactive app or an open editor/sheet keeps its focus", async () => {
  const fixture_ = fixture();
  fixture_.host.isWindowFocused = async () => false;
  const dispose = attachVisualizerFocus(fixture_.host);
  await tick();
  assert.deepEqual(fixture_.events, []);
  fixture_.host.isWindowFocused = async () => true;
  fixture_.host.canRestore = () => false;
  fixture_.resize();
  await tick();
  assert.deepEqual(fixture_.events, []);
  dispose();
});

test("opening a sheet while focus IPC is in flight prevents later focus theft", async () => {
  const fixture_ = fixture();
  let resolve;
  fixture_.host.isWindowFocused = () => new Promise((done) => { resolve = done; });
  const dispose = attachVisualizerFocus(fixture_.host);
  fixture_.host.canRestore = () => false;
  resolve(true);
  await tick();
  assert.deepEqual(fixture_.events, []);
  dispose();
});

test("unmount cancels pending focus and cleans up late listener registration", async () => {
  const fixture_ = fixture();
  let resolveFocus;
  let resolveListener;
  fixture_.host.isWindowFocused = () => new Promise((done) => { resolveFocus = done; });
  fixture_.host.onResize = () => new Promise((done) => { resolveListener = done; });
  const dispose = attachVisualizerFocus(fixture_.host);
  dispose();
  resolveFocus(true);
  resolveListener(() => fixture_.events.push("unlisten"));
  await tick();
  assert.deepEqual(fixture_.events, ["unlisten"]);
});

test("failed native focus leaves the loop alive and reports only once", async () => {
  const fixture_ = fixture();
  fixture_.host.focusWebview = async () => { throw new Error("unavailable"); };
  const dispose = attachVisualizerFocus(fixture_.host);
  await tick();
  fixture_.resize();
  await tick();
  assert.deepEqual(fixture_.events, ["error"]);
  dispose();
});
