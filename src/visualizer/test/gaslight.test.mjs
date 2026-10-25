import assert from "node:assert/strict";
import { test } from "node:test";
import { GaslightField } from "../gaslight.ts";
import { DEFAULT_GASLIGHT, GASLIGHT_PALETTES, normalizeGaslight } from "../gaslightSettings.ts";
import { PixelLimiter } from "../pixelLimiter.ts";
import { worstChangesPerWindow } from "../flash.ts";
import { nextStyle, VISUALIZER_STYLES } from "../styles.ts";
import { loadVisualizerPrefs, saveVisualizerPrefs } from "../../preferences/playbackPrefs.ts";

const audio = (level = 0.5) => ({ bands: new Float32Array(64).fill(level), energy: level });
const settings = (changes) => ({ ...DEFAULT_GASLIGHT, ...changes });

function luminance(pixels) {
  let sum = 0;
  for (let i = 0; i < pixels.length; i += 4) {
    sum += (pixels[i] / 255) ** 2.2 * 0.2126 + (pixels[i + 1] / 255) ** 2.2 * 0.7152 + (pixels[i + 2] / 255) ** 2.2 * 0.0722;
  }
  return sum / (pixels.length / 4);
}

test("settings fill defaults and reject invalid stored values", () => {
  for (const value of [null, undefined, [], "old", 42]) assert.deepEqual(normalizeGaslight(value), DEFAULT_GASLIGHT);
  assert.deepEqual(normalizeGaslight({ palette: "other", trails: 7, glow: -1, height: NaN, autopilot: "true" }), {
    ...DEFAULT_GASLIGHT, trails: 1, glow: 0,
  });
  assert.equal(normalizeGaslight({ glow: Infinity }).glow, DEFAULT_GASLIGHT.glow);
  assert.equal(normalizeGaslight({ palette: "inverted", autopilot: true }).palette, "inverted");
});

test("old preferences migrate and complete Gaslight preferences survive a reload", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  let saved = JSON.stringify({ style: "lines", microphone: true, microphoneDevice: "test mic", limitFlashing: false });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: () => saved, setItem: (_, value) => { saved = value; },
  } });
  try {
    const migrated = loadVisualizerPrefs();
    assert.equal(migrated.style, "lines");
    assert.equal(migrated.microphone, true);
    assert.equal(migrated.limitFlashing, false);
    assert.deepEqual(migrated.gaslight, DEFAULT_GASLIGHT);
    const changed = { ...migrated, style: "gaslight", gaslight: { palette: "aurora", trails: 0.3, glow: 0.8, height: 0.9, autopilot: true } };
    saveVisualizerPrefs(changed);
    assert.deepEqual(loadVisualizerPrefs(), changed);
    saved = JSON.stringify({ style: "missing", limitFlashing: "false", microphone: "true", gaslight: { height: 3 } });
    const repaired = loadVisualizerPrefs();
    assert.equal(repaired.style, "classic");
    assert.equal(repaired.limitFlashing, true);
    assert.equal(repaired.microphone, false);
    assert.equal(repaired.gaslight.height, 1);
    saved = "{broken";
    assert.deepEqual(loadVisualizerPrefs().gaslight, DEFAULT_GASLIGHT);
    // A caller cannot mutate the defaults by adjusting a returned layout.
    repaired.gaslight.height = 0;
    assert.equal(loadVisualizerPrefs().gaslight.height, DEFAULT_GASLIGHT.height);
  } finally {
    if (original) Object.defineProperty(globalThis, "localStorage", original);
    else delete globalThis.localStorage;
  }
});

test("Gaslight participates in both directions of keyboard style cycling", () => {
  assert.equal(nextStyle("classic", 1), "gaslight");
  assert.equal(nextStyle("gaslight", -1), "classic");
  assert.equal(new Set(VISUALIZER_STYLES).size, VISUALIZER_STYLES.length);
  let style = "gaslight";
  for (let i = 0; i < VISUALIZER_STYLES.length; i++) style = nextStyle(style, 1);
  assert.equal(style, "gaslight");
});

test("every palette has exactly mirrored opaque output in one reused, bounded buffer", () => {
  const field = new GaslightField();
  const first = field.pixels;
  for (const palette of GASLIGHT_PALETTES) {
    const pixels = field.render(1 / 60, audio(), settings({ palette }));
    assert.equal(pixels, first);
    assert.ok(pixels.length <= 512 * 256 * 4);
    for (let y = 0; y < field.height / 2; y++) {
      const top = y * field.width * 4;
      const bottom = (field.height - 1 - y) * field.width * 4;
      assert.deepEqual(pixels.subarray(top, top + field.width * 4), pixels.subarray(bottom, bottom + field.width * 4));
    }
    for (let i = 3; i < pixels.length; i += 4) assert.equal(pixels[i], 255);
  }
});

test("a frequency impulse stays in its band and height controls its distance from the center", () => {
  const signal = audio(0);
  signal.bands[19] = 0.9;
  signal.energy = 0.8;
  const expectedX = 19 / 63;
  const peaks = [];
  for (const height of [0.1, 0.9]) {
    const field = new GaslightField();
    const pixels = field.render(0, signal, settings({ height, glow: 0, trails: 0 }));
    let greatest = 0;
    let peak = [0, 0];
    for (let y = 0; y < field.height / 2; y++) for (let x = 0; x < field.width; x++) {
      const value = pixels[(y * field.width + x) * 4 + 1];
      if (value > greatest) { greatest = value; peak = [x, y]; }
    }
    assert.ok(Math.abs(peak[0] / field.width - expectedX) < 0.02);
    peaks.push(peak[1]);
  }
  assert.ok(peaks[1] < peaks[0] - 30, `${peaks}`);
});

test("palette backgrounds and hue families follow the three reference looks", () => {
  const frames = GASLIGHT_PALETTES.map((palette) => {
    const field = new GaslightField();
    return field.render(0, audio(0.7), settings({ palette }));
  });
  // A sub-visible halo can reach the edge; backgrounds retain black/white contrast.
  assert.ok([...frames[0].slice(0, 3)].every((value) => value < 4));
  assert.ok([...frames[1].slice(0, 3)].every((value) => value < 4));
  assert.ok([...frames[2].slice(0, 3)].every((value) => value > 251));
  let emberWarm = 0, auroraCool = 0, invertedColor = 0;
  for (let i = 0; i < frames[0].length; i += 4) {
    if (frames[0][i] > frames[0][i + 2] + 30) emberWarm++;
    if (frames[1][i + 2] > frames[1][i] + 30) auroraCool++;
    if (frames[2][i] > frames[2][i + 1] + 30) invertedColor++;
  }
  assert.ok(emberWarm > 1000 && auroraCool > 1000 && invertedColor > 1000);
});

test("trails outlive a stopped signal, decay, and their lifetime is adjustable", () => {
  const remaining = [];
  for (const trails of [0, 1]) {
    const field = new GaslightField();
    const prefs = settings({ trails });
    for (let i = 0; i < 30; i++) field.render(1 / 30, audio(0.6), prefs);
    const lit = luminance(field.pixels);
    for (let i = 0; i < 18; i++) field.render(1 / 30, audio(0), prefs);
    const trail = luminance(field.pixels);
    remaining.push(trail);
    assert.ok(trail < lit);
    for (let i = 0; i < 120; i++) field.render(1 / 30, audio(0), prefs);
    assert.ok(luminance(field.pixels) <= trail);
    if (trails > 0) assert.ok(luminance(field.pixels) < trail * 0.25);
  }
  assert.ok(remaining[1] > remaining[0] * 4, `${remaining}`);
});

test("fixed-time histories agree across 30, 60 and 120 Hz", () => {
  const frames = [30, 60, 120].map((fps) => {
    const field = new GaslightField();
    const signal = audio();
    field.render(0, signal, DEFAULT_GASLIGHT);
    for (let i = 1; i <= fps; i++) {
      signal.energy = 0.5 + 0.2 * Math.sin(i / fps * 4);
      for (let band = 0; band < 64; band++) signal.bands[band] = 0.5 + 0.2 * Math.sin(i / fps * 4 + band * 0.3);
      field.render(1 / fps, signal, DEFAULT_GASLIGHT);
    }
    return field.pixels;
  });
  for (const frame of frames.slice(1)) {
    let difference = 0;
    for (let i = 0; i < frame.length; i++) difference += Math.abs(frame[i] - frames[0][i]);
    assert.ok(difference / frame.length < 0.6, `average pixel difference ${difference / frame.length}`);
  }
});

test("malformed audio and stalled clocks cannot poison the field", () => {
  const field = new GaslightField();
  const invalid = { bands: [NaN, Infinity, -1, 900, undefined], energy: NaN };
  for (const dt of [NaN, Infinity, -1, 10000, 0]) field.render(dt, invalid, DEFAULT_GASLIGHT);
  const valid = field.render(1 / 60, audio(), DEFAULT_GASLIGHT);
  assert.ok(luminance(valid) > 0);
  assert.ok(luminance(valid) < 1);
});

test("software limiter controls a rapid strobe in linear light and can be disabled", () => {
  const limiter = new PixelLimiter();
  const pixels = new Uint8ClampedArray(32 * 16 * 4);
  const input = [], shown = [];
  for (let frame = 0; frame < 360; frame++) {
    pixels.fill(Math.floor(frame / 4) % 2 ? 220 : 65);
    input.push(luminance(pixels));
    limiter.apply(pixels, 1 / 60, true);
    shown.push(luminance(pixels));
  }
  assert.ok(worstChangesPerWindow(input, 1000 / 60) > 6);
  assert.ok(worstChangesPerWindow(shown.slice(60), 1000 / 60) <= 6);
  pixels.fill(200);
  limiter.apply(pixels, 1 / 60, false);
  assert.equal(pixels[0], 200);
  assert.equal(limiter.gain, 1);
  assert.equal(limiter.veil, 0);
  limiter.apply(pixels, 1 / 60, true);
  assert.equal(pixels[0], 200, "re-enabling starts from the currently shown brightness");
});
