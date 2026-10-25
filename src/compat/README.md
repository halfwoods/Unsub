# High Sierra browser baseline

The Intel compatibility candidate targets **macOS 10.13.6 with Safari 13.1.2**,
which provides the system WebKit used by Tauri. Installing another browser does
not update that webview. This is a build/runtime target, not a claim that the
candidate has been run on High Sierra.

- Apple lists Safari 13.1.2 for High Sierra: <https://support.apple.com/en-us/103116>.
- ResizeObserver arrived in Safari 13.1: <https://webkit.org/blog/10247/new-webkit-features-in-safari-13-1/>.
- Vite's production JS and CSS targets are both `safari13.1`. Dependencies are
  transformed too. This does not polyfill browser APIs, and the development
  server is not the compatibility build.

`bootstrap.ts` runs before React and Tauri modules. It measures flex gap (a
`CSS.supports("gap", ...)` check would incorrectly accept grid-only support),
marks missing CSS capabilities and installs `queueMicrotask` only if absent.
The fallback preserves Promise job ordering and reports callback errors outside
the Promise chain. It does not alter working native implementations.

`theme/compat.css` supplies spacing, focus, artwork and visualizer-text fallbacks.
Keep every new flex-gap surface covered, including labels with anonymous text
children, responsive overrides, wrapped controls and inherited flex layouts.
Grid gap remains native. The fallback uses static CSS and one startup measurement;
it adds no per-row observer or work to virtualized lists. Wrapped controls have
small bottom margins to separate their last line from the following controls.

Artist mosaics use an explicit two-cover class instead of `:has`. Window-button
`color-mix` declarations are inside `@supports`; merely placing variable-dependent
values after a fallback can invalidate that fallback at computed-value time.

## Runtime API audit

The production application and installed React, Tauri, Zustand and virtualizer
code were inspected alongside the emitted assets. Required APIs fit the 13.1
baseline: ResizeObserver, pointer events/capture, MutationObserver, MessageChannel,
DOMRect, URL/URLSearchParams, DOMParser, CSS.escape, Proxy, Map/Set/WeakMap/WeakSet,
typed arrays, Promise/finally/allSettled, Object.entries/values/fromEntries,
Array.flat/flatMap and the current Intl number/date formatting options.

Special cases:

- Tauri channel error handling calls `queueMicrotask`; bootstrap covers it.
  The installed Tauri 2.12.4 `init.js`, `core.js`, `ipc.js`, Wry initialization
  scripts and installed plug-in initialization scripts contain no such call.
  The optional global Tauri bundle only uses it inside a channel callback error
  handler, not during module initialization; `withGlobalTauri` is disabled here.
  Thus the required callsites run after the frontend bootstrap. Re-audit injected
  initialization code when upgrading Tauri, since Vite does not transform it.
- Tauri's optional `Resource[Symbol.asyncDispose]` method becomes an unused
  `undefined` property without that symbol. Unsub explicitly calls normal APIs
  and does not use `await using`; no disposal polyfill is needed for current code.
  Re-audit if explicit resource management is introduced.
- React's newer view-transition APIs are unused by Unsub; React catches a missing
  `startViewTransition` and performs normal mutation/layout work. Network
  Information access is feature-tested. Those optional APIs are not polyfilled.
- The only event-listener `signal` option in app code is in the Linux-only native
  menu cleanup branch. The macOS path waits for the menu Promise instead.
- WebGL2 is optional. Visualizers have a Canvas2D path; both require live testing
  on the target GPU/OS in addition to current-host tests.

Future dependencies or uses of newer built-ins need another runtime audit;
lowering a compiler target alone is insufficient.

## Reproduce verification

```sh
node --test src/compat/test/*.test.mjs src/browser/test/*.test.mjs
pnpm build
```

The tests check missing-capability behavior, native API preservation, microtask
semantics, flex-gap/focus fallback coverage, artwork failure transitions, and an
actual production Vite transform of newer dependency syntax and overlay CSS.

With `pnpm dev` running, open `/src/compat/preview.html`. The isolated fixture
uses real application CSS, synthetic content and no library/API/storage access.
Switch between native and forced fallback layouts, then wide/narrow widths.
It measures spacing, square artwork, mosaic height and overflow. Tab to the mix
button to check its visible focus and play affordance. Forcing legacy mode
disables native flex gap inside the fixture while leaving grid gap intact.

The fixture was exercised in the current host's Chrome at 958px and 398px inner
widths, with passing geometry checks, visible keyboard focus and no console
warnings/errors. This demonstrates fallback behavior on a modern engine. It
does **not** replace launching the packaged app on actual High Sierra/WebKit 13.1.
