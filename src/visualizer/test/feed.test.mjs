import assert from "node:assert/strict";
import { test } from "node:test";

import { MockFeed, TauriFeed, BAND_COUNT, WAVEFORM_LENGTH } from "../feed.ts";

test("frames have the shape the analyzer promises", () => {
  const frame = new MockFeed().frameAt(3210);
  assert.equal(frame.bands.length, BAND_COUNT);
  assert.equal(frame.peaks.length, BAND_COUNT);
  assert.equal(frame.waveformL.length, WAVEFORM_LENGTH);
  assert.equal(frame.waveformR.length, WAVEFORM_LENGTH);
  assert.ok(frame.bands.every((band) => band >= 0 && band <= 1));
  assert.ok(frame.peaks.every((peak, i) => peak >= frame.bands[i] && peak <= 1));
  assert.ok([...frame.waveformL, ...frame.waveformR].every((sample) => sample >= -1 && sample <= 1));
  assert.ok(frame.rms >= 0 && frame.rms <= 1 && frame.onset >= 0 && frame.onset <= 1);
  assert.equal(typeof frame.beat, "boolean");
  assert.equal(frame.tMs, 3210);
});

test("the same time gives the same frame", () => {
  const a = new MockFeed().frameAt(5000);
  const b = new MockFeed().frameAt(5000);
  assert.deepEqual(a, b);
});

test("there is a beat every half second at 120 BPM, a stronger one every fourth", () => {
  const feed = new MockFeed({ bpm: 120, fps: 60 });
  const beats = [];
  for (let tMs = 0; tMs < 8000; tMs += 1000 / 60) {
    const frame = feed.frameAt(tMs);
    if (frame.beat) beats.push({ tMs, strength: frame.beatStrength });
  }
  assert.ok(beats.length >= 15 && beats.length <= 17, `${beats.length} beats in 8 s`);
  for (let i = 1; i < beats.length; i++) {
    assert.ok(Math.abs(beats[i].tMs - beats[i - 1].tMs - 500) < 20, `gap ${beats[i].tMs - beats[i - 1].tMs}`);
  }
  const strengths = new Set(beats.map((beat) => beat.strength));
  assert.deepEqual([...strengths].sort(), [0.7, 1]);
  assert.equal(beats.filter((beat) => beat.strength === 1).length, 4);
});

test("the bass kicks on the beat and the highs come between", () => {
  const feed = new MockFeed({ bpm: 120 });
  const onBeat = feed.frameAt(2000 + 20);
  const between = feed.frameAt(2000 + 380);
  const mean = (frame, from, to) => frame.bands.slice(from, to).reduce((a, b) => a + b, 0) / (to - from);
  assert.ok(mean(onBeat, 0, 8) > mean(between, 0, 8));
  assert.ok(mean(feed.frameAt(2250 + 20), 48, 64) > mean(onBeat, 48, 64));
});

test("the spectrum drifts over time", () => {
  const feed = new MockFeed();
  const a = feed.frameAt(10000).bands;
  const b = feed.frameAt(13100).bands;
  const difference = a.reduce((sum, band, i) => sum + Math.abs(band - b[i]), 0);
  assert.ok(difference > 1);
});

test("the tempo is unknown for the first moments, as with the real analyzer", () => {
  const feed = new MockFeed({ bpm: 128 });
  assert.equal(feed.frameAt(500).tempoBpm, null);
  assert.equal(feed.frameAt(5000).tempoBpm, 128);
});

test("listeners get frames until they unsubscribe", () => {
  const feed = new MockFeed();
  const heard = [];
  const stop = feed.onFrame((frame) => heard.push(frame.tMs));
  feed.emit(0);
  feed.emit(16);
  stop();
  feed.emit(32);
  assert.deepEqual(heard, [0, 16]);
});

test("it can be started and stopped like the real feed", async () => {
  const feed = new MockFeed({ fps: 200 });
  let count = 0;
  feed.onFrame(() => count++);
  feed.start();
  assert.ok(feed.running);
  // Timers are at the mercy of a busy machine, so wait for frames rather than for a time.
  for (let waited = 0; count < 3 && waited < 2000; waited += 10) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  feed.stop();
  const atStop = count;
  assert.ok(atStop >= 3, `${atStop} frames`);
  assert.ok(!feed.running);
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.equal(count, atStop);
});

/** A backend that records what it is asked and lets the test decide when each request finishes. */
function fakeBackend() {
  const calls = [];
  const gates = [];
  const gate = () => new Promise((resolve) => gates.push(resolve));
  return {
    calls,
    emit: (frame) => calls.findLast((call) => call.name === "start").onFrame(frame),
    release: () => gates.splice(0).forEach((resolve) => resolve()),
    start(onFrame, source) {
      calls.push({ name: "start", onFrame, source });
      return gate();
    },
    stop() {
      calls.push({ name: "stop" });
      return gate();
    },
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const MIC = { type: "microphone", device: "usb-1" };

test("the backend starts with the first listener and stops with the last", async () => {
  const backend = fakeBackend();
  const feed = new TauriFeed(backend, MIC, assert.fail);
  assert.deepEqual(backend.calls, [], "nothing runs until somebody listens");

  const first = feed.onFrame(() => {});
  const second = feed.onFrame(() => {});
  await settle();
  assert.deepEqual(backend.calls.map((call) => call.name), ["start"]);
  assert.deepEqual(backend.calls[0].source, MIC);

  first();
  await settle();
  assert.equal(backend.calls.length, 1, "one listener is still left");
  second();
  second();
  backend.release();
  await settle();
  assert.deepEqual(backend.calls.map((call) => call.name), ["start", "stop"]);
});

test("every listener gets every frame the backend sends", async () => {
  const backend = fakeBackend();
  const feed = new TauriFeed(backend, { type: "music" }, assert.fail);
  const a = [];
  const b = [];
  feed.onFrame((frame) => a.push(frame.tMs));
  const stopB = feed.onFrame((frame) => b.push(frame.tMs));
  await settle();

  backend.emit(new MockFeed().frameAt(10));
  stopB();
  backend.emit(new MockFeed().frameAt(20));

  assert.deepEqual(a, [10, 20]);
  assert.deepEqual(b, [10]);
});

test("a stop waits for the start before it, and a start for the stop before it", async () => {
  const backend = fakeBackend();
  const feed = new TauriFeed(backend, { type: "music" }, assert.fail);

  // Hidden and shown again before the backend has answered even the first request.
  feed.onFrame(() => {})();
  feed.onFrame(() => {});
  await settle();
  assert.deepEqual(backend.calls.map((call) => call.name), ["start"]);

  backend.release();
  await settle();
  assert.deepEqual(backend.calls.map((call) => call.name), ["start", "stop"]);
  backend.release();
  await settle();
  assert.deepEqual(backend.calls.map((call) => call.name), ["start", "stop", "start"]);
});

test("a backend that cannot start is reported, and the feed carries on", async () => {
  const errors = [];
  let attempts = 0;
  const backend = {
    start: () => (++attempts === 1 ? Promise.reject("microphone access was denied") : Promise.resolve()),
    stop: () => Promise.resolve(),
  };
  const feed = new TauriFeed(backend, MIC, (error) => errors.push(error));

  feed.onFrame(() => {})();
  feed.onFrame(() => {});
  await settle();
  await settle();

  assert.deepEqual(errors, ["microphone access was denied"]);
  assert.equal(attempts, 2, "the second listener still got its start");
});
