import assert from "node:assert/strict";
import { test } from "node:test";

import { FlashLimiter, worstChangesPerWindow } from "../flash.ts";

const FRAME_MS = 1000 / 60;

/** Runs a luminance signal through the limiter and returns what would be shown. */
function limited(signal, options) {
  const limiter = new FlashLimiter(options);
  return signal.map((luminance) => {
    const limit = limiter.update(FRAME_MS, luminance);
    return FlashLimiter.shown(luminance, limit);
  });
}

function square(hz, low, high, seconds) {
  const frames = Math.round((seconds * 1000) / FRAME_MS);
  return Array.from({ length: frames }, (_, i) =>
    Math.floor((i * FRAME_MS * hz * 2) / 1000) % 2 === 0 ? low : high,
  );
}

function sine(hz, low, high, seconds) {
  const frames = Math.round((seconds * 1000) / FRAME_MS);
  return Array.from({ length: frames }, (_, i) =>
    low + ((high - low) * (0.5 + 0.5 * Math.sin((2 * Math.PI * hz * i * FRAME_MS) / 1000))),
  );
}

// Three flashes a second is six changes.
const MOST_CHANGES = 6;

test("the counter sees what the guideline would", () => {
  // 8 Hz is 16 changes a second, give or take a frame at the window's edges.
  const eightHertz = worstChangesPerWindow(square(8, 0.1, 0.6, 3), FRAME_MS);
  assert.ok(eightHertz >= 15 && eightHertz <= 17, `${eightHertz}`);
  assert.ok(worstChangesPerWindow(square(8, 0.1, 0.18, 3), FRAME_MS) === 0, "under 10% is not a change");
  assert.ok(worstChangesPerWindow(square(1, 0.1, 0.6, 5), FRAME_MS) <= 3);
});

for (const hz of [3.5, 4, 6, 10, 15, 30]) {
  test(`a ${hz} Hz strobe from dark to bright is limited to under three flashes a second`, () => {
    const input = square(hz, 0.05, 0.6, 6);
    assert.ok(worstChangesPerWindow(input, FRAME_MS) > MOST_CHANGES, "the input really does flash");
    const shown = limited(input);
    // Allow the average to settle for a second.
    assert.ok(worstChangesPerWindow(shown.slice(60), FRAME_MS) <= MOST_CHANGES);
  });
}

test("a pulsing sine is limited too", () => {
  const shown = limited(sine(5, 0.1, 0.5, 6));
  assert.ok(worstChangesPerWindow(shown.slice(60), FRAME_MS) <= MOST_CHANGES);
});

test("beats on a dark picture, four a second, are limited", () => {
  const frames = 60 * 6;
  const input = Array.from({ length: frames }, (_, i) => {
    const sinceBeat = (i % 15) / 15;
    return 0.08 + 0.45 * Math.exp(-sinceBeat * 9);
  });
  assert.ok(worstChangesPerWindow(input, FRAME_MS) > MOST_CHANGES);
  assert.ok(worstChangesPerWindow(limited(input).slice(60), FRAME_MS) <= MOST_CHANGES);
});

test("noise cannot get past it", () => {
  let seed = 7;
  const random = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
  const input = Array.from({ length: 60 * 6 }, () => 0.05 + 0.6 * random());
  assert.ok(worstChangesPerWindow(limited(input).slice(60), FRAME_MS) <= MOST_CHANGES);
});

test("a swing is held to twice the deadband around the slow average", () => {
  const shown = limited(square(6, 0.05, 0.6, 6)).slice(120);
  const swing = Math.max(...shown) - Math.min(...shown);
  assert.ok(swing <= 2 * 0.035 + 0.02, `swing ${swing}`);
});

test("a slow swell passes through almost untouched", () => {
  // From dark to bright over four seconds.
  const frames = 60 * 6;
  const input = Array.from({ length: frames }, (_, i) =>
    Math.min(0.6, 0.1 + (0.5 * i) / (60 * 4)),
  );
  const shown = limited(input);
  for (let i = 0; i < frames; i++) {
    assert.ok(Math.abs(shown[i] - input[i]) < 0.08, `frame ${i}: ${input[i]} became ${shown[i]}`);
  }
});

test("a fade to black follows the music", () => {
  const input = Array.from({ length: 60 * 5 }, (_, i) => Math.max(0, 0.5 - i / (60 * 3)));
  const shown = limited(input);
  assert.ok(shown.at(-1) < 0.1, "it ends dark");
});

test("a steady picture is left alone", () => {
  const limiter = new FlashLimiter();
  for (let i = 0; i < 300; i++) {
    const limit = limiter.update(FRAME_MS, 0.3);
    assert.equal(limit.gain, 1);
    assert.equal(limit.veil, 0);
  }
});

test("the first frame is not treated as a flash from black", () => {
  const limiter = new FlashLimiter();
  const limit = limiter.update(FRAME_MS, 0.7);
  assert.equal(limit.gain, 1);
});

test("it never dims below its floor or lifts past its ceiling", () => {
  const limiter = new FlashLimiter({ minGain: 0.5, maxVeil: 0.05 });
  for (let i = 0; i < 60; i++) limiter.update(FRAME_MS, 0.02);
  const bright = limiter.update(FRAME_MS, 1);
  assert.ok(bright.gain >= 0.5);
  for (let i = 0; i < 60; i++) limiter.update(FRAME_MS, 0.9);
  const dark = limiter.update(FRAME_MS, 0);
  assert.ok(dark.veil <= 0.05);
});
