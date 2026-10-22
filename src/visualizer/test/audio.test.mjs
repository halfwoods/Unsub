import assert from "node:assert/strict";
import { test } from "node:test";

import { AudioState } from "../audio.ts";
import { MockFeed, BAND_COUNT, WAVEFORM_LENGTH } from "../feed.ts";

const STEP = 1 / 60;

function frame(overrides = {}) {
  return {
    tMs: 0,
    bands: new Array(BAND_COUNT).fill(0),
    peaks: new Array(BAND_COUNT).fill(0),
    waveformL: new Array(WAVEFORM_LENGTH).fill(0),
    waveformR: new Array(WAVEFORM_LENGTH).fill(0),
    rms: 0,
    beat: false,
    beatStrength: 0,
    onset: 0,
    tempoBpm: null,
    ...overrides,
  };
}

/** Runs the state for `seconds`, handing it a fresh copy of `make()` on every step. */
function run(audio, seconds, nowMs, make) {
  for (let i = 0; i < Math.round(seconds / STEP); i++) {
    nowMs += STEP * 1000;
    if (make) audio.receive(make(), nowMs);
    audio.update(STEP, nowMs);
  }
  return nowMs;
}

test("bands rise quickly and fall slowly", () => {
  const audio = new AudioState();
  const loud = () => frame({ bands: new Array(BAND_COUNT).fill(1) });
  let now = run(audio, 1, 0, loud); // settle live, then reach full height
  assert.ok(audio.bands[10] > 0.95, `${audio.bands[10]}`);

  now = run(audio, 3 * STEP, now, () => frame());
  assert.ok(audio.bands[10] > 0.5, "after 50 ms of silence it has barely moved");
  now = run(audio, 1, now, () => frame());
  assert.ok(audio.bands[10] < 0.05, `${audio.bands[10]}`);
  assert.ok(audio.bands.every((band) => band >= 0 && band <= 1));
});

test("values outside 0..1 are clamped", () => {
  const audio = new AudioState();
  run(audio, 1, 0, () => frame({ bands: new Array(BAND_COUNT).fill(5), rms: 9 }));
  assert.ok(audio.bands.every((band) => band <= 1));
  assert.ok(audio.rms <= 1);
  run(audio, 1, 1000, () => frame({ bands: new Array(BAND_COUNT).fill(-5) }));
  assert.ok(audio.bands.every((band) => band >= 0));
});

test("peak caps hold, then fall faster and faster, and never below the bar", () => {
  const audio = new AudioState();
  let now = run(audio, 1, 0, () => frame({ bands: new Array(BAND_COUNT).fill(1) }));
  now = run(audio, 0.1, now, () => frame());
  assert.ok(audio.peaks[5] > 0.99, `still held after 100 ms: ${audio.peaks[5]}`);
  now = run(audio, 0.6, now, () => frame());
  assert.ok(audio.peaks[5] < 0.95 && audio.peaks[5] > 0.2, `falling: ${audio.peaks[5]}`);
  const earlier = audio.peaks[5];
  now = run(audio, 0.2, now, () => frame());
  const fallen = earlier - audio.peaks[5];
  assert.ok(fallen > 0.1, "accelerating");
  run(audio, 3, now, () => frame());
  assert.ok(audio.peaks.every((peak, i) => peak >= audio.bands[i]));
});

test("a beat becomes two envelopes that decay at their own rates", () => {
  const audio = new AudioState();
  audio.receive(frame({ beat: true, beatStrength: 0.8 }), 0);
  assert.equal(audio.beatCount, 1);
  assert.ok(Math.abs(audio.kick - 0.8) < 1e-9);
  audio.update(0.075, 10);
  assert.ok(audio.kick < 0.4 && audio.kick > 0.2, `kick ${audio.kick}`);
  assert.ok(audio.beat > 0.5, `beat ${audio.beat}`);
  for (let i = 0; i < 30; i++) audio.update(STEP, 20 + i);
  assert.ok(audio.beat < 0.1 && audio.kick < 0.001);
  audio.receive(frame({ beat: false }), 100);
  assert.equal(audio.beatCount, 1, "a frame without a beat is not one");
});

test("a very weak beat still shows", () => {
  const audio = new AudioState();
  audio.receive(frame({ beat: true, beatStrength: 0 }), 0);
  assert.ok(audio.kick >= 0.35);
});

test("when frames stop it eases into an idle drift, and back", () => {
  const audio = new AudioState();
  const music = () => new MockFeed().frameAt(1234);
  let now = run(audio, 1.5, 0, music);
  assert.ok(audio.live > 0.95 && !audio.idle);

  now = run(audio, 0.3, now);
  assert.ok(audio.live < 1, "the music has gone stale");
  now = run(audio, 4, now);
  assert.ok(audio.idle, `live ${audio.live}`);
  const drift = Math.max(...audio.bands);
  assert.ok(drift > 0.01 && drift < 0.3, `it still moves, quietly: ${drift}`);
  assert.ok(audio.rms < 0.15);
  assert.ok(Math.max(...audio.waveL.map(Math.abs)) < 0.15);

  run(audio, 1, now, music);
  assert.ok(!audio.idle && audio.live > 0.7, `live ${audio.live}`);
  assert.ok(Math.max(...audio.bands) > 0.3);
});

test("time keeps advancing while idle, so the drift moves", () => {
  const audio = new AudioState();
  run(audio, 1, 0);
  const before = audio.bands[3];
  run(audio, 2.1, 1000);
  assert.notEqual(audio.bands[3], before);
  assert.ok(audio.time > 3);
});

test("the four rows of the texture are the waveforms, the bands and the peaks", () => {
  const audio = new AudioState();
  run(audio, 1, 0, () => new MockFeed().frameAt(777));
  const t = audio.texture;
  assert.equal(t.length, WAVEFORM_LENGTH * 4);
  assert.deepEqual([...t.slice(0, WAVEFORM_LENGTH)], [...audio.waveL]);
  assert.deepEqual([...t.slice(WAVEFORM_LENGTH, WAVEFORM_LENGTH * 2)], [...audio.waveR]);
  assert.deepEqual([...t.slice(WAVEFORM_LENGTH * 2, WAVEFORM_LENGTH * 2 + BAND_COUNT)], [...audio.bands]);
  assert.deepEqual([...t.slice(WAVEFORM_LENGTH * 3, WAVEFORM_LENGTH * 3 + BAND_COUNT)], [...audio.peaks]);
});

test("low, mid and high follow their share of the spectrum", () => {
  const audio = new AudioState();
  const bands = new Array(BAND_COUNT).fill(0);
  for (let i = 0; i < 8; i++) bands[i] = 1;
  run(audio, 1, 0, () => frame({ bands }));
  assert.ok(audio.low > 0.9 && audio.mid < 0.05 && audio.high < 0.05);
});

test("drawing from a frame allocates next to nothing", { skip: typeof globalThis.gc !== "function" }, () => {
  const audio = new AudioState();
  const music = new MockFeed().frameAt(1234);
  audio.receive(music, 0);
  for (let i = 0; i < 20000; i++) audio.update(STEP, i * 16.7);
  globalThis.gc();
  const before = process.memoryUsage().heapUsed;
  const steps = 100000;
  for (let i = 0; i < steps; i++) audio.update(STEP, 20000 * 16.7 + i * 16.7);
  const perStep = (process.memoryUsage().heapUsed - before) / steps;
  assert.ok(perStep < 100, `${perStep} bytes a frame`);
});

test("energy follows how loud the music is, from calm to lively", () => {
  const audio = new AudioState();
  const at = (rms) => () => frame({ rms });
  let now = run(audio, 3, 0, at(0.3));
  const loud = audio.energy;
  now = run(audio, 3, now, at(0.1));
  const middling = audio.energy;
  now = run(audio, 3, now, at(0.02));
  const quiet = audio.energy;
  assert.ok(loud > 0.8, `loud ${loud}`);
  assert.ok(middling > 0.35 && middling < 0.65, `middling ${middling}`);
  assert.ok(quiet < 0.2, `quiet ${quiet}`);
  now = run(audio, 6, now);
  assert.ok(audio.energy > 0.05 && audio.energy < 0.2, `idle ${audio.energy}`);
});

test("energy swells in a fraction of a second and settles over a second or two", () => {
  const audio = new AudioState();
  let now = run(audio, 3, 0, () => frame({ rms: 0.02 }));
  now = run(audio, 0.3, now, () => frame({ rms: 0.3 }));
  assert.ok(audio.energy > 0.6, `swell ${audio.energy}`);
  now = run(audio, 0.3, now, () => frame({ rms: 0.02 }));
  assert.ok(audio.energy > 0.3, `still settling ${audio.energy}`);
  run(audio, 3, now, () => frame({ rms: 0.02 }));
  assert.ok(audio.energy < 0.2);
});

test("a spark is an onset that stands out of the ones around it, and is gone in a fifth of a second", () => {
  const audio = new AudioState();
  let now = run(audio, 2, 0, () => frame({ onset: 0.08 }));
  assert.ok(audio.spark < 0.05, `a steady onset is not a spark: ${audio.spark}`);
  audio.receive(frame({ onset: 0.7 }), now);
  audio.update(STEP, (now += STEP * 1000));
  assert.ok(audio.spark > 0.7, `spark ${audio.spark}`);
  now = run(audio, 0.3, now, () => frame({ onset: 0.08 }));
  assert.ok(audio.spark < 0.05, `gone: ${audio.spark}`);
});

test("a spark is not lost when two frames arrive between two draws", () => {
  const audio = new AudioState();
  let now = run(audio, 2, 0, () => frame({ onset: 0.08 }));
  audio.receive(frame({ onset: 0.9 }), now);
  audio.receive(frame({ onset: 0.08 }), now + 8);
  audio.update(STEP, (now += STEP * 1000));
  assert.ok(audio.spark > 0.8, `spark ${audio.spark}`);
});

test("a thump is the bass standing above its own average, whatever the song's level", () => {
  const audio = new AudioState();
  const bass = (level) => {
    const bands = new Array(BAND_COUNT).fill(0.2);
    for (let i = 0; i < 8; i++) bands[i] = level;
    return frame({ bands });
  };
  let now = run(audio, 4, 0, () => bass(0.7));
  assert.ok(audio.thump < 0.05, `steady bass is not a thump: ${audio.thump}`);
  now = run(audio, 0.1, now, () => bass(0.95));
  assert.ok(audio.thump > 0.5, `thump ${audio.thump}`);
  now = run(audio, 5, now, () => bass(0.95));
  assert.ok(audio.thump < 0.05, `a thump that does not stop is the new level: ${audio.thump}`);
});
