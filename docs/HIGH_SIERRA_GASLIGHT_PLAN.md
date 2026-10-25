# High Sierra compatibility and Gaslight visualizer

Status: implementation complete; final CI/package verification in progress. Actual High Sierra runtime remains an external release gate. Base `origin/main` pulled before edits at `9b2eb5f`; branch `mac-mini/high-sierra-gaslight`. The checkout was clean. User requested both features on 2026-10-25 and authorized routine implementation decisions and agent delegation. Repository instructions authorize small commits, push and PR; publishing another release is outside this task.

## Outcomes and decisions

1. Produce an Intel build targeting fully updated macOS High Sierra 10.13.6 with Safari 13.1.2. Apple Silicon remains macOS 11+, and Windows remains supported. Preserve one Intel binary/update platform rather than create a legacy fork. Runtime support must not be advertised as verified until tested on 10.13.
2. Add an original built-in **Gaslight** visualizer that closely follows the published GasLight references: mirrored frequency ridges, glowing vertical plumes, fine history trails and a central light band. Provide Ember, Aurora and Inverted palettes, adjustable trails/glow/height and optional slow autopilot. It uses Unsub's own code and audio analysis; it does not load, copy or redistribute the original plug-in, shaders or artwork.
3. Preserve classic styling, audio playback, library data, existing visualizers, keyboard/fullscreen controls, casting behavior and flashing limits. The new visualizer must retain its character on the Canvas2D fallback available without WebGL2.

The compatibility baseline is the final High Sierra/Safari updates, not the original Safari 11 shipped with 10.13. The current host is an Apple Silicon Mac running macOS 15.8.1. A question about access to a High Sierra test machine is pending; it does not block implementation/build inspection. No other computer or filesystem is assumed available.

## Evidence

- Every existing release tag through 0.5.0 declares macOS 11. There is no existing High Sierra release.
- Tauri documents macOS 10.13 as its baseline: <https://v2.tauri.app/distribute/macos-application-bundle/#minimum-system-version>.
- Apple lists Safari 13.1.2 for High Sierra: <https://support.apple.com/en-us/103116>; High Sierra 10.13.6 update: <https://support.apple.com/en-us/106395>.
- Vite currently has no explicit browser target; its default is newer than High Sierra's WebKit. CSS uses flex gap, `:has`, `color-mix`, `inset`, container units and newer focus selectors. WebKit documents ResizeObserver in Safari 13.1: <https://webkit.org/blog/10247/new-webkit-features-in-safari-13-1/>.
- Native microphone permission selectors require 10.14; the cloud-download script names Music, whereas High Sierra uses iTunes. CPAL's process-tap path references 14.2 APIs. The platform certificate verifier reaches `SecTrustEvaluateWithError`, introduced in 10.14. These require real compatibility paths, not only a lower bundle version.
- The FFmpeg composite action hardcodes deployment target 11.0 and omits it from the cache key. The bundling script checks library minimums but must also check the main executable.
- Current visualizers use WebGL2; a missing GPU renderer falls back to Spectrum for every style. That path has inaccurate overlay/notice assumptions and does not pass through the existing flash limiter.
- GasLight's developer page and screenshots: <https://steelskies.com/gaslight/>. Reference images were inspected from the developer's site. Its downloadable v2 bundle contains Intel i386/x86_64 code and old iTunes/Carbon/AGL/OpenGL dependencies. It was inspected without installation or execution. No source/assets from that package enter this repository.

## Architecture and ownership

### Native and dependency compatibility

- Keep the normal Intel app/update archive usable on 10.13; set deployment targets explicitly per architecture (Intel 10.13.6, matching the documented OS baseline) in release configuration and CI. Apple Silicon uses 11.0.
- Guard APIs by OS availability before messaging classes/selectors. On pre-10.14 macOS, microphone use follows the older OS permission model; later systems keep the current explicit permission flow.
- Select iTunes for pre-10.15 media download requests and Music on later systems.
- Resolve optional process-tap functions dynamically and refuse the optional modern loopback path below its supported OS. Normal input/output devices must not acquire a hard dependency on 14.2 symbols/classes.
- Retain system certificate trust verification on older macOS using the appropriate older evaluation API. Fail closed on verification errors. Prefer narrowly scoped, licensed dependency patches over removing TLS validation or changing the trust policy. Record any vendored patches and their reason.
- No library schema migration or media-file modification is needed.

### Browser compatibility

- Set production JavaScript/CSS output to Safari 13.1 explicitly. Load a minimal capability bootstrap before React/Tauri consumers; polyfill only missing required primitives.
- Use capability-gated flex-gap fallbacks and explicit CSS equivalents for unsupported selectors/values. Modern browsers retain the existing layout. Keep focus indicators and narrow-window behavior.
- Replace structural `:has` dependence with state/classes owned by the relevant component. Audit emitted production assets as well as source.

### Gaslight rendering and preferences

- Add `gaslight` to the existing style registry and native menu; keep existing style identifiers/defaults stable.
- Create a bounded, deterministic spectrum-history/pixel model fed by the existing 64 log-spaced bands and smoothed audio envelopes. Time-based decay must behave consistently across refresh rates. Preallocate persistent data and bound work independently of display resolution.
- Share that model between a WebGL2 texture adapter and a Canvas2D adapter. Upscale a bounded internal field smoothly; preserve visible frequency placement and mirrored ridges rather than substitute generic bars.
- Palette references: Ember uses orange/yellow ridges and red plumes on black; Aurora uses cyan ridges with a hot red/orange center; Inverted uses a white background with magenta/red/yellow trails.
- Extend `VisualizerPrefs` with validated defaults for palette, trails, glow, height and autopilot. Existing saved settings migrate by filling defaults; corrupt/out-of-range values cannot reach the renderer. Settings live in the existing Playback → Visualizer preferences and persist per computer through localStorage.
- Select the appropriate software renderer when WebGL2 is absent; accurately report the displayed style, overlay contrast and fallback state. Preserve flashing protection on both renderers, song-change overlays, idle behavior, visibility suspension, full screen, Escape and arrow-key cycling. Casting continues to show Album.

### Build and distribution

- Thread architecture-specific deployment targets through FFmpeg build/cache/bundling. Cache entries must distinguish minimum OS and be rejected when incompatible.
- Validate the app executable and every bundled library's architecture and minimum OS. Audit strong imports of known newer APIs so lowering metadata cannot hide a loader failure.
- Add Intel compatibility build coverage and focused script tests to CI using the repository's pinned Rust toolchain. Existing macOS ARM/Linux/Windows checks remain.
- Keep version 0.5.0 and existing published assets untouched during development. Build/test artifacts are candidates, not a new release. Record the actual OS test gap in documentation and the PR.

## Implementation sequence and ledger

| Item | Owner / primary files | Acceptance | Status |
| --- | --- | --- | --- |
| Native API availability and old iTunes integration | Native agent: `src-tauri/src/visualizer.rs`, `collection.rs` | No unavailable selector call on 10.13; later OS behavior retained | Implemented; focused tests passed |
| Audio/TLS dependency compatibility | Native agent: Cargo manifests/lock, narrowly scoped vendor patches if needed | No required 14.2 audio import; pre-10.14 trust evaluation remains secure | Implemented; isolated TLS/audio tests and rebuilt-symbol check passed |
| Safari 13.1 frontend output and layout | Web agent: `vite.config.ts`, compatibility bootstrap/styles/tests, targeted component CSS | Production bundle targets 13.1; missing capabilities retain functional layout/focus | Implemented; production build, 55 browser/compat tests and live forced fallback checks passed |
| Shared Gaslight rendering and adapters | Visualizer agent: `src/visualizer/` | Three reference-informed looks on GPU and software paths; bounded/time-correct rendering | Implemented; model regressions and live GPU/software checks passed |
| Preferences, menu and runtime integration | Visualizer agent: visualizer files, playback preferences, Preferences.tsx, menu.rs | Saved settings, migration, keyboard/fullscreen, clear fallback and safe flashing | Implemented; native keyboard, reset, restart persistence and fullscreen focus regression verified |
| Deployment targets, cache and binary audits | Root: `.github/`, `scripts/` | Intel 10.13.6/ARM 11 artifacts audited; cache keys include deployment target | Implemented; 13 binary fixtures and fresh ARM executable audit passed; Intel CI pending |
| Automated regression/build validation | All, integrated by root | Relevant frontend/Rust tests, production build, format/lint, binary checks pass | Local frontend 150 passed/1 existing skip; Rust workspace 2,601 passed/7 ignored; CI pending |
| Live UI and compatibility evidence | Root | Native/preview rendering, software fallback, palettes/settings, resize/keyboard/idle inspected | Native generated-file playback, idle, settings, keyboard and 823×423 window verified; preview policy block documented |
| Actual High Sierra runtime | Requires 10.13 Intel environment | Launch, import/playback, library UI, TLS/update check and new visualizer work | Environment unavailable here; user asked |
| Documentation, commits and PR | Root | Accurate support matrix, patch notes, final evidence and limitations, clean branch/PR | Committed and pushed; draft PR #4 open; final evidence update pending |

## Verification

- Renderer regressions: finite bounded output, frequency position, mirrored geometry, palette differences, time-based trails across frame rates, pause/resume, invalid inputs, preference migration/clamping and flashing limits. Tests assert behavior, not source snapshots.
- Compatibility regressions: forced missing browser capabilities, actual production syntax/transform checks, unsupported-layout fallback coverage, deployment target/cache propagation and binary audit failure fixtures.
- Run the complete relevant frontend suites and production build, targeted native/dependency checks and workspace Rust validation. Use the pinned CI Rust version for strict linting; distinguish newer local-toolchain advisories.
- Exercise the actual rendered app or isolated visualizer preview with deterministic audio and generated local files only. Inspect all palettes on GPU and forced software rendering, resize/narrow windows, keyboard cycling, fullscreen/Escape, settings persistence, idle decay and playback continuity. Observe performance and cleanup; do not infer UI success from static code.
- Intel compilation and Mach-O inspection prove build/link properties only. They do not substitute for executing on High Sierra. A supported public release requires the actual runtime matrix above; if no host is available, deliver the implemented/tested candidate and explicitly identify that remaining gate.

## Delivery boundaries

- No permission changes, personal-library imports, original GasLight installation, or third-party source copying.
- No lowering certificate security or weakening tests to obtain a passing build.
- Preserve modern appearance and source/playback behaviors established in 0.5.0.
- Do not publish a new version or overwrite release assets as part of this implementation request.
- Any plan adjustment must preserve the two requested outcomes and update this ledger with evidence.

## Current validation record

- Local host: Apple M1, macOS 15.8.1, Node 26.11.1, Rust 1.99.1. Full frontend production build passed with the existing main-chunk size warning. Final frontend suites: 150 passed, one existing optional allocation skip. Rust workspace: 2,601 passed, seven existing ignores; format check passed. The pinned Rust 1.93.1 CI remains authoritative for strict Clippy; the newer local compiler reports existing unrelated lints.
- Fifteen focused native/dependency tests cover permission guards, old iTunes integration, old/new certificate evaluation, hostname/root/date failures and optional process-tap availability. The certificate fixtures are synthetic, network fetching is disabled and no keychain is changed.
- Thirteen binary-audit tests cover thin/universal binaries, minimum versions, architectures, strong versus weak imports, stripped symbol tables, dyld bind streams and chained imports. Known unsupported import encodings and unknown required loader commands fail closed. Legacy targets also reject newer required export-trie/chained-fixup commands even when the deployment metadata is lowered. The parser also inspected all fourteen Mach-O files from existing Intel/ARM 0.5.0 archives. The freshly rebuilt ARM executable passed its minimum/architecture/import audit.
- Browser checks completed before a later saved-origin-policy denial: all three Gaslight palettes across GPU/software paths; software strobe fixture; forced legacy layout at 958px and 398px with visible keyboard focus. After the denial, all localhost browser access stopped. These are modern Chrome checks, not High Sierra execution.
- Native bundled application, loading its packaged files without the blocked preview server: imported and played only `/tmp/unsub-gaslight-qa/gaslight.flac`, a generated tone/sweep fixture. Gaslight reacted to playback and settled to its idle line when paused. The native menu and Cmd+T opened Gaslight. Arrow keys changed styles.
- Native preferences: Aurora and a keyboard-adjusted 51% trail value survived a full quit/relaunch. Autopilot toggled by keyboard, and Reset Gaslight restored Ember/70% trails/65% glow/70% height/autopilot off. Limit flashing stayed enabled and microphone stayed disabled. At approximately 823×423 logical pixels, Playback preferences scrolled to every control while OK stayed visible.
- Live fullscreen testing discovered a first-responder loss after native fullscreen transitions. Added guarded webview/content focus restoration and five async regressions. Rebuilt native app then passed F → Right → Left → Escape without clicking the canvas; Escape closed the visualizer and restored the window. Open editors/dialogs and inactive applications are excluded from focus restoration.
- CPU field benchmark on this M1: roughly 1.16–1.22 ms median and 1.35–1.60 ms p95 per 384×256 frame. This excludes compositing/limiting/GPU work and is not an old-Intel performance claim.
- Draft [PR #4](https://github.com/halfwoods/Unsub/pull/4) is open. Initial CI found a Windows-only path-separator assumption in the audit test; the assertion now uses the platform-native relative path. Final CI results and the Intel candidate link will be recorded after completion.

- QA cleanup: the generated library and both test webview data directories were archived under `/tmp/unsub-high-sierra-native-state`; the normal app data folder had been absent before testing. The sole indexed file was the generated fixture. The native app and task-owned Vite server were closed. No personal library or music files were modified.
