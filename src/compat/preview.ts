/** Isolated, deterministic CSS QA. No Tauri calls, library access or saved state.
 * Open /src/compat/preview.html in the dev server. This exercises fallbacks on a
 * modern browser; it does not emulate Safari's JavaScript engine. */
import "../theme/base.css";
import "../theme/compat.css";
import "../chrome/Toolbar.css";
import "../chrome/TrafficLights.css";
import "../browser/AlbumGrid.css";
import "../browser/ViewOptions.css";
import "../preferences/Preferences.css";
import "../sidebar/Sidebar.css";
import "../mixes/MixesView.css";
import "../info/GetInfo.css";

const root = document.getElementById("root")!;
const styles = document.createElement("style");
styles.textContent = `html,body,#root{height:auto;overflow:auto;background:#eee}body{padding:16px}h1{font-size:16px}h2{font-size:13px;margin:10px 0}button{cursor:pointer}iframe{display:block;border:1px solid #aaa;margin-top:12px;background:white;max-width:100%}.fixture-section{background:white;border:1px solid #bbb;padding:10px;margin:12px 0}.fixture-row{position:static;transform:none;left:auto;top:auto}.toolbar-right.fixture-row{justify-content:flex-start}.fixture-swatch{width:40px;height:24px;background:#aabbd0}.fixture-readout{white-space:pre-wrap;font:11px/1.5 Menlo,monospace}.traffic-lights.fixture-row{position:static;width:14px}.fixture-artwork{width:120px}.fixture-mosaic{width:120px;height:120px}.fixture-mosaic>span{background:#7c8da8;border:1px solid white}.fixture-label{margin-bottom:10px}.view-options-column>label{overflow:hidden;white-space:nowrap}.view-options-column{padding-left:0;padding-right:0}`;
document.head.append(styles);
const query = new URLSearchParams(location.search);
if (!query.has("sample")) {
  root.innerHTML = `<h1>High Sierra layout fallback fixture</h1><p>Modern browser rendering with missing layout capabilities forced. This is not an old-WebKit runtime test.</p>
    <div><button class="push-button" id="wide">Wide 960px</button> <button class="push-button" id="narrow">Narrow 400px</button>
    <button class="push-button" id="legacy">Use legacy fallbacks</button> <button class="push-button" id="modern">Use native layout</button></div>
    <iframe title="Compatibility sample" width="960" height="950" src="?sample=1&legacy=1"></iframe>`;
  const frame = root.querySelector("iframe")!;
  document.getElementById("wide")!.onclick = () => { frame.width = "960"; };
  document.getElementById("narrow")!.onclick = () => { frame.width = "400"; };
  document.getElementById("legacy")!.onclick = () => { frame.src = "?sample=1&legacy=1"; };
  document.getElementById("modern")!.onclick = () => { frame.src = "?sample=1"; };
} else {
  const legacy = query.has("legacy");
  root.innerHTML = `<h1>${legacy ? "Forced legacy" : "Native"} layout</h1>
    <section class="fixture-section"><h2>Toolbar and window controls</h2><div class="toolbar-right fixture-row"><button class="fixture-swatch" aria-label="First toolbar control"></button><button class="fixture-swatch" aria-label="Second toolbar control"></button></div>
    <div class="traffic-lights fixture-row" data-active="true"><button class="traffic-light traffic-light--close" aria-label="Close fixture"></button><button class="traffic-light traffic-light--minimize" aria-label="Minimize fixture"></button></div></section>
    <section class="fixture-section"><h2>Preferences and columns</h2><label class="prefs-check fixture-label"><input type="checkbox">Keep media folder organized</label>
    <label class="view-options-search">Find a column<input type="search" aria-label="Find a column"></label>
    <div class="view-options-column"><label><input type="checkbox">Album Artist</label><button class="push-button">←</button><button class="push-button">→</button><input type="number" aria-label="Column width" value="160"></div>
    <div class="view-options-presets"><label>Layout<select><option>Library Cleanup</option></select></label><button class="push-button">Restore Defaults</button><button class="push-button">Save Layout</button></div></section>
    <section class="fixture-section"><h2>Square artwork and two-cover mosaic</h2><div class="fixture-artwork"><div class="artwork-pane-image"><span class="artwork-pane-empty">No Artwork</span></div></div>
    <button class="album-tile" aria-label="Open fixture artist"><span class="artist-mosaic artist-mosaic--multiple artist-mosaic--two fixture-mosaic"><span>Cover A</span><span>Cover B</span></span><span class="album-tile-title">An artist with a long display name</span></button></section>
    <section class="fixture-section"><h2>Keyboard focus and text wrapping</h2><button class="mix-tile" aria-label="Play fixture mix"><span class="mix-mosaic" style="width:100px;height:70px"><span class="mix-play">Play</span></span></button>
    <p class="info-where">/Test/AnExtremelyLongLocalFolderNameWithoutSpacesForWrapping/AndAnotherLongFolderName/AnAlbumWithAnExtremelyLongName.flac</p></section>
    <pre class="fixture-readout" aria-live="polite"></pre>`;
  if (legacy) {
    document.documentElement.classList.add("no-flex-gap", "no-focus-visible", "no-container-units", "no-aspect-ratio");
    // Disable native flex gap only in this fixture. Grid gaps remain functional,
    // just as on Safari 13.1. The application never rewrites element styles.
    for (const element of document.querySelectorAll<HTMLElement>("*")) {
      if (["flex", "inline-flex"].includes(getComputedStyle(element).display)) element.style.setProperty("gap", "0px", "important");
    }
  }
  const rect = (selector: string) => document.querySelector(selector)!.getBoundingClientRect();
  const report = () => {
    const first = rect(".toolbar-right > :first-child"), second = rect(".toolbar-right > :last-child");
    const top = rect(".traffic-lights > :first-child"), bottom = rect(".traffic-lights > :last-child");
    const artwork = rect(".artwork-pane-image"), cover = rect(".fixture-mosaic > :first-child");
    const checks = {
      "Toolbar gap 14px": Math.abs(second.left - first.right - 14) < 0.5,
      "Window-control gap 4px": Math.abs(bottom.top - top.bottom - 4) < 0.5,
      "Square artwork 120px": Math.abs(artwork.width - artwork.height) < 0.5 && artwork.height === 120,
      "Two-cover mosaic height 120px": cover.height === 120,
      "No horizontal page overflow": document.documentElement.scrollWidth <= innerWidth,
    };
    root.querySelector("pre")!.textContent = `Viewport: ${innerWidth}px\n${Object.entries(checks).map(([label, passed]) => `${passed ? "PASS" : "FAIL"} ${label}`).join("\n")}\nFocused: ${document.activeElement?.getAttribute("aria-label") || document.activeElement?.tagName}`;
  };
  requestAnimationFrame(report);
  window.addEventListener("resize", report);
  document.addEventListener("focusin", report);
}
