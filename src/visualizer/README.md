# Gaslight visualizer

Gaslight is an original renderer informed by the published [GasLight screenshots](https://steelskies.com/gaslight/): mirrored spectrum ridges, vertical glow and fine history trails. It does not load the iTunes plug-in or reuse its code, shaders or artwork.

`gaslight.ts` produces a fixed 384 × 256 RGBA field with a bounded 96-sample history. Both the WebGL2 adapter and software renderer use this field. Trail sampling runs at 30 Hz with interpolation, independently of display refresh rate. Canvas2D uses ordinary image data and scaling; it does not require WebGL2 or Canvas filters. Both paths apply the existing linear-luminance flashing limiter.

The View → Visualizer menu and Playback preferences share the `gaslight` style. Preferences provide Ember, Aurora and Inverted colors, trails, glow, spectrum height, slow autopilot and Reset Gaslight. Autopilot varies height and glow within the selected palette. Settings are validated under `unsub.visualizer.gaslight`; older preferences receive defaults and invalid values are repaired on load. No library migration is involved.

## Validation recorded 2026-10-25

- `node --test src/visualizer/test/*.test.mjs`: 72 passed, one existing optional allocation test skipped. Ten new regressions cover migration/reload, malformed settings/audio, frequency placement, height, mirrored pixels, palette contrast, trail decay, frame-rate independence, keyboard style cycling and software limiting.
- `pnpm build`: passed. The existing main-chunk size warning remains. The rendering implementation stays in the lazy visualizer chunk; settings are a separate small module.
- Live Chrome inspection, completed before browser access was subsequently blocked by the computer's saved origin policy: Ember through WebGL2; Aurora and Inverted through forced Canvas2D. Mirroring, trails, palette contrast and accurate software labels were visible. Dark backgrounds used light song text; Inverted used dark text.
- The software preview's 8 Hz strobe stress run reported zero qualifying whole-picture luminance changes per second for each palette, with limiting enabled. This is evidence for that fixture, not a guarantee about every region or input. A separate GPU stress result could not be reliably distinguished and is not claimed.

## CPU-only benchmark

Run `node src/visualizer/test/bench-gaslight.mjs`. It opens no browser, audio device, media file or library. After 120 warm-up frames it measures 600 deterministic frames per palette.

Apple M1, arm64, macOS 15.8.1, Node 26.11.1:

| Palette | Median ms/frame | 95th percentile ms/frame |
| --- | ---: | ---: |
| Ember | 1.156 | 1.601 |
| Aurora | 1.214 | 1.349 |
| Inverted | 1.218 | 1.400 |

The reused output is 393,216 bytes. These timings measure only the CPU field; they exclude pixel limiting, browser compositing, GPU work and display scaling. They do not establish performance on an Intel High Sierra Mac.

## Remaining verification

The root native pass subsequently verified short-window preferences, keyboard controls, reset, restart persistence, playback/idle behavior and fullscreen/Escape. It found and fixed a fullscreen focus regression, with five additional async tests. A live GPU benchmark and actual High Sierra runtime remain unavailable. The native application pass and actual Intel High Sierra launch/playback tests are recorded separately in the [implementation plan](../../docs/HIGH_SIERRA_GASLIGHT_PLAN.md). No further localhost browser access was attempted after the origin-policy block.
