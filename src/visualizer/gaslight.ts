/**
 * An original spectrum/history renderer inspired by the published GasLight pictures.
 * No plug-in code, shaders or artwork are used. The same bounded field feeds both renderers.
 */
import type { GaslightPalette, GaslightSettings } from "./gaslightSettings";

// Work and storage do not grow with display size. Fine traces remain smooth when upscaled.
export const GASLIGHT_WIDTH = 384;
export const GASLIGHT_HEIGHT = 256;
const HALF = GASLIGHT_HEIGHT / 2;
const HISTORY = 96;
const SAMPLE_STEP = 1 / 30;
const LUT_SIZE = 4096;

export interface GaslightAudio {
  readonly bands: ArrayLike<number>;
  readonly energy: number;
}

const finiteUnit = (value: number) => Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : 0;

/** Fixed-step history with interpolation; trails have the same lifetime at 30, 60 or 120 Hz. */
export class GaslightField {
  readonly width = GASLIGHT_WIDTH;
  readonly height = GASLIGHT_HEIGHT;
  /** Owned, reused RGBA bytes. Do not retain a frame without copying it. */
  readonly pixels = new Uint8ClampedArray(GASLIGHT_WIDTH * GASLIGHT_HEIGHT * 4);
  private readonly spectrum = new Float32Array(GASLIGHT_WIDTH);
  private readonly previous = new Float32Array(GASLIGHT_WIDTH);
  private readonly history = new Float32Array(HISTORY * GASLIGHT_WIDTH);
  private readonly historyEnergy = new Float32Array(HISTORY);
  private readonly lines = new Float32Array(GASLIGHT_WIDTH * HALF);
  private readonly bloom = new Float32Array(GASLIGHT_WIDTH * HALF);
  private readonly plume = new Float32Array(GASLIGHT_WIDTH * HALF);
  private readonly light = new Float32Array(LUT_SIZE);
  private age = 0;
  private clock = 0;
  private head = -1;
  private count = 0;
  private previousEnergy = 0;

  constructor() {
    for (let i = 0; i < LUT_SIZE; i++) this.light[i] = 1 - Math.exp(-i / 768);
  }

  render(dt: number, audio: GaslightAudio, settings: Readonly<GaslightSettings>): Uint8ClampedArray {
    const step = Number.isFinite(dt) ? Math.max(0, Math.min(0.1, dt)) : 0;
    this.clock += step;
    const { spectrum, previous, history, lines, bloom, plume } = this;
    const energy = finiteUnit(audio.energy);
    const bands = audio.bands;
    const lastBand = Math.max(0, bands.length - 1);
    for (let x = 0; x < GASLIGHT_WIDTH; x++) {
      // Linear interpolation preserves the position of every log-spaced input band.
      const band = x / (GASLIGHT_WIDTH - 1) * lastBand;
      const left = Math.floor(band);
      const blend = band - left;
      const level = finiteUnit(bands[left]) * (1 - blend) + finiteUnit(bands[Math.min(left + 1, lastBand)]) * blend;
      spectrum[x] = Math.pow(level, 0.8);
    }
    if (this.head < 0) {
      previous.set(spectrum);
      this.previousEnergy = energy;
      this.capture(spectrum, spectrum, energy, energy, 1);
    }
    let untilSample = SAMPLE_STEP - this.age;
    while (untilSample <= step + 1e-9) {
      const blend = step > 0 ? untilSample / step : 1;
      this.capture(previous, spectrum, this.previousEnergy, energy, blend);
      untilSample += SAMPLE_STEP;
    }
    this.age = Math.max(0, this.age + step - Math.floor((this.age + step + 1e-9) / SAMPLE_STEP) * SAMPLE_STEP);
    previous.set(spectrum);
    this.previousEnergy = energy;

    lines.fill(0);
    plume.fill(0);
    // Autopilot is continuous, bounded and deliberately slow; it never changes the background.
    const drift = settings.autopilot ? Math.sin(this.clock / 13) : 0;
    const height = (0.22 + settings.height * 0.62) * (1 + drift * 0.08);
    const lifetime = 0.14 + settings.trails * 1.25;
    const travel = 9 + settings.trails * 13;
    for (let trace = this.count - 1; trace >= 0; trace--) {
      const age = trace * SAMPLE_STEP + this.age;
      const fade = Math.exp(-age / lifetime);
      if (fade < 0.025) continue;
      const index = (this.head - trace + HISTORY) % HISTORY;
      const start = index * GASLIGHT_WIDTH;
      const strength = fade * (0.3 + this.historyEnergy[index] * 0.7);
      for (let x = 1; x < GASLIGHT_WIDTH - 1; x++) {
        const level = history[start + x];
        if (level < 0.004) continue;
        const edge = Math.min(1, x / 8, (GASLIGHT_WIDTH - 1 - x) / 8);
        const y = 2 + level * (HALF - 12) * height + age * travel * (0.35 + level * 0.65);
        const row = Math.floor(y);
        if (row >= HALF - 2) continue;
        const fraction = y - row;
        const amount = strength * edge * (0.16 + level * 0.84);
        // Keep individual historical ridges legible instead of filling a solid bar.
        const at = row * GASLIGHT_WIDTH + x;
        lines[at] = Math.max(lines[at], amount * (1 - fraction));
        lines[at + GASLIGHT_WIDTH] = Math.max(lines[at + GASLIGHT_WIDTH], amount * fraction);
      }
    }
    // The live ridge stays sharp even between history samples.
    for (let x = 1; x < GASLIGHT_WIDTH - 1; x++) {
      const level = spectrum[x];
      const y = 2 + level * (HALF - 12) * height;
      const row = Math.floor(y);
      const fraction = y - row;
      const amount = (0.25 + energy * 0.75) * Math.min(1, x / 8, (GASLIGHT_WIDTH - 1 - x) / 8) * (0.12 + level * 0.88);
      const at = row * GASLIGHT_WIDTH + x;
      lines[at] += amount * (1 - fraction);
      lines[at + GASLIGHT_WIDTH] += amount * fraction;
    }

    // Vertical decay from every ridge makes luminous plumes. Two inexpensive separable
    // passes add the wider halo; no Canvas filters, per-pixel allocations or GPU-only effects.
    const glow = settings.glow * (1 + drift * 0.1);
    const vertical = settings.palette === "ember" ? 0.92 + glow * 0.065 : 0.84 + glow * 0.1;
    for (let x = 0; x < GASLIGHT_WIDTH; x++) {
      let up = 0;
      let down = 0;
      for (let y = 0; y < HALF; y++) {
        const at = y * GASLIGHT_WIDTH + x;
        up = up * vertical + lines[at] * 0.17;
        plume[at] = up;
        const reverse = (HALF - 1 - y) * GASLIGHT_WIDTH + x;
        down = down * vertical + lines[reverse] * 0.17;
        bloom[reverse] = down;
      }
    }
    for (let y = 0; y < HALF; y++) {
      let left = 0;
      let right = 0;
      const start = y * GASLIGHT_WIDTH;
      for (let x = 0; x < GASLIGHT_WIDTH; x++) {
        const at = start + x;
        plume[at] += bloom[at];
        left = left * 0.67 + plume[at] * 0.33;
        bloom[at] = left;
      }
      for (let x = GASLIGHT_WIDTH - 1; x >= 0; x--) {
        const at = start + x;
        right = right * 0.67 + plume[at] * 0.33;
        bloom[at] = (bloom[at] + right) * 0.5;
      }
    }
    this.paint(settings.palette, glow, energy);
    return this.pixels;
  }

  private capture(from: Float32Array, to: Float32Array, fromEnergy: number, toEnergy: number, blend: number): void {
    this.head = (this.head + 1) % HISTORY;
    this.count = Math.min(HISTORY, this.count + 1);
    const start = this.head * GASLIGHT_WIDTH;
    for (let x = 0; x < GASLIGHT_WIDTH; x++) this.history[start + x] = from[x] + (to[x] - from[x]) * blend;
    this.historyEnergy[this.head] = fromEnergy + (toEnergy - fromEnergy) * blend;
  }

  private paint(palette: GaslightPalette, glow: number, energy: number): void {
    const { pixels, lines, plume, bloom, light } = this;
    const tone = (value: number) => light[Math.min(LUT_SIZE - 1, Math.max(0, Math.floor(value * 768)))];
    for (let y = 0; y < HALF; y++) {
      const core = Math.exp(-y / 2.8) * (0.32 + energy * 0.68);
      const centerGap = Math.min(1, y / 3);
      for (let x = 0; x < GASLIGHT_WIDTH; x++) {
        const at = y * GASLIGHT_WIDTH + x;
        const ridge = lines[at] * 2.7;
        const halo = (plume[at] * 0.42 + bloom[at] * 1.7) * glow;
        const edge = Math.min(1, x / 8, (GASLIGHT_WIDTH - 1 - x) / 8);
        const center = core * edge;
        let r: number;
        let g: number;
        let b: number;
        if (palette === "aurora") {
          const cool = tone(ridge + halo * 0.8) * centerGap;
          r = tone(center * 5 + ridge * 0.18 + halo * 0.18);
          g = Math.min(1, cool * 0.94 + tone(center * 0.7));
          b = Math.min(1, cool + tone(center * 0.06));
        } else if (palette === "inverted") {
          r = 1 - tone(ridge * 2 + halo * 0.2) * centerGap * 0.8;
          g = 1 - Math.min(0.98, tone(ridge * 2 + halo * 3) * centerGap + tone(center * 0.12));
          b = 1 - Math.min(0.98, tone(ridge * 1.7 + halo * 1.3 + center * 6));
        } else {
          r = tone(ridge * 1.5 + halo * 3) * centerGap;
          g = tone(ridge * 0.83 + halo * 0.28) * centerGap;
          b = tone(ridge * ridge * 0.2 + halo * 0.015) * centerGap;
        }
        const top = ((HALF - 1 - y) * GASLIGHT_WIDTH + x) * 4;
        const bottom = ((HALF + y) * GASLIGHT_WIDTH + x) * 4;
        pixels[top] = pixels[bottom] = r * 255;
        pixels[top + 1] = pixels[bottom + 1] = g * 255;
        pixels[top + 2] = pixels[bottom + 2] = b * 255;
        pixels[top + 3] = pixels[bottom + 3] = 255;
      }
    }
  }
}
