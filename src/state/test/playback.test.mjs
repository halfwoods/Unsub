import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { registerHooks } from "node:module";

const storeUrl = new URL("../store.ts", import.meta.url).href;
let readTrack;
registerHooks({ resolve(specifier, context, nextResolve) {
  if (context.parentURL === storeUrl && specifier === "../api/commands") {
    return { url: "data:text/javascript,export const libraryTrack=(id)=>globalThis.__readPlaybackTrack(id);export const libraryChapters=()=>Promise.resolve([]);", shortCircuit: true };
  }
  // The application uses extensionless imports; Node's type stripping needs the extension.
  if (context.parentURL?.startsWith(new URL("../../../", import.meta.url).href) && specifier.startsWith(".") && !/\.[a-z]+$/i.test(specifier)) {
    return nextResolve(`${specifier}.ts`, context);
  }
  return nextResolve(specifier, context);
} });
globalThis.__readPlaybackTrack = (id) => readTrack(id);
const { useApp } = await import(storeUrl);
const initial = useApp.getState();
const first = { id: 1, title: "First", durationMs: 1000 };
const second = { id: 2, title: "Second", durationMs: 2000 };
const wait = () => new Promise((resolve) => setImmediate(resolve));

beforeEach(() => {
  readTrack = async (id) => ({ id, title: `Track ${id}`, durationMs: 3000 });
  useApp.setState({ ...initial, playingTracks: [first, second], playingSource: { type: "library" },
    playback: { ...initial.playback, state: "playing", trackId: 1, positionMs: 950, durationMs: 1000 }, nowPlaying: first });
});

test("a track start moves the song list indicator without a separate state change", () => {
  useApp.getState().applyPlayerEvent({ type: "trackStarted", trackId: 2 });
  const { playback, nowPlaying } = useApp.getState();
  assert.equal(playback.trackId, 2);
  assert.equal(playback.state, "playing");
  assert.equal(playback.positionMs, 0);
  assert.equal(playback.durationMs, 2000);
  assert.equal(nowPlaying, second);
});

test("late metadata cannot replace the track that is now playing", async () => {
  let resolveOld;
  readTrack = () => new Promise((resolve) => { resolveOld = resolve; });
  useApp.getState().applyPlayerEvent({ type: "trackStarted", trackId: 3 });
  assert.equal(useApp.getState().nowPlaying, null);
  useApp.getState().applyPlayerEvent({ type: "trackStarted", trackId: 2 });
  resolveOld({ id: 3, title: "Old metadata", durationMs: 3000 });
  await wait();
  assert.equal(useApp.getState().nowPlaying, second);
});

test("stopping while metadata loads keeps the current track empty", async () => {
  let resolveTrack;
  readTrack = () => new Promise((resolve) => { resolveTrack = resolve; });
  useApp.getState().applyPlayerEvent({ type: "trackStarted", trackId: 3 });
  useApp.getState().applyPlayerEvent({ type: "stateChanged", ...initial.playback });
  resolveTrack({ id: 3, title: "Stopped track", durationMs: 3000 });
  await wait();
  assert.equal(useApp.getState().nowPlaying, null);
});

test("an uncached track gets its metadata after the indicator moves", async () => {
  useApp.getState().applyPlayerEvent({ type: "trackStarted", trackId: 3 });
  assert.equal(useApp.getState().playback.trackId, 3);
  await wait();
  assert.equal(useApp.getState().nowPlaying.id, 3);
});

test("late library metadata cannot overwrite a device track with the same ID", async () => {
  let resolveTrack;
  readTrack = () => new Promise((resolve) => { resolveTrack = resolve; });
  useApp.getState().applyPlayerEvent({ type: "trackStarted", trackId: 3 });
  const deviceTrack = { id: 3, title: "Device track", durationMs: 4000 };
  useApp.setState({ playingSource: { type: "device", deviceId: "ipod" }, nowPlaying: deviceTrack });
  resolveTrack({ id: 3, title: "Library track", durationMs: 3000 });
  await wait();
  assert.equal(useApp.getState().nowPlaying, deviceTrack);
});
