import assert from "node:assert/strict";
import { test } from "node:test";

import { MockFeed, BAND_COUNT, WAVEFORM_LENGTH } from "../feed.ts";

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
