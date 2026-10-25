// CPU model only. No browser, canvas, GPU, audio device, media file or library is opened.
// Run: node src/visualizer/test/bench-gaslight.mjs
import { arch, platform, release, cpus } from "node:os";
import { GaslightField } from "../gaslight.ts";
import { DEFAULT_GASLIGHT, GASLIGHT_PALETTES } from "../gaslightSettings.ts";

const warmup = 120;
const measured = 600;
const signal = { bands: new Float32Array(64), energy: 0.7 };
const results = [];
for (const palette of GASLIGHT_PALETTES) {
  const field = new GaslightField();
  const settings = { ...DEFAULT_GASLIGHT, palette };
  const timings = [];
  for (let frame = 0; frame < warmup + measured; frame++) {
    for (let band = 0; band < 64; band++) signal.bands[band] = 0.4 + 0.3 * Math.sin(frame / 23 + band / 5);
    const start = performance.now();
    field.render(1 / 60, signal, settings);
    if (frame >= warmup) timings.push(performance.now() - start);
  }
  timings.sort((a, b) => a - b);
  results.push({
    palette,
    medianMs: Number(timings[Math.floor(measured / 2)].toFixed(3)),
    p95Ms: Number(timings[Math.floor(measured * 0.95)].toFixed(3)),
    outputSize: [field.width, field.height],
    outputBytes: field.pixels.byteLength,
  });
}
console.log(JSON.stringify({
  note: "CPU field only; excludes browser compositing, pixel limiting, GPU work and display scaling.",
  machine: { cpu: cpus()[0]?.model, platform: platform(), kernel: release(), arch: arch(), node: process.version },
  warmupFrames: warmup, measuredFrames: measured, results,
}, null, 2));
