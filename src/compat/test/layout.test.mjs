import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { build, resolveConfig } from "vite";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const compat = readFileSync(join(root, "src/theme/compat.css"), "utf8");
function* cssFiles(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) yield* cssFiles(join(directory, entry.name));
    else if (entry.name.endsWith(".css") && entry.name !== "compat.css") yield join(directory, entry.name);
  }
}
// These use real CSS Grid gap, which Safari 13.1 supports. The device options
// selector inherits flex from device-box-content and must still be audited.
const gridGap = new Set([".device-facts", ".capacity", ".capacity-legend-item", ".cleanup-list li", ".info-grid", ".info-facts", ".prefs-grid"]);

test("every flex-gap surface has an explicit legacy fallback, including responsive rules", () => {
  const uncovered = [];
  for (const file of cssFiles(join(root, "src"))) {
    const css = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const [, selectorList, declarations] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      if (!/(?:^|;)\s*(?:gap|row-gap|column-gap)\s*:/.test(declarations)) continue;
      for (const selector of selectorList.trim().split(/\s*,\s*/)) {
        if (gridGap.has(selector)) continue;
        if (!compat.includes(`.no-flex-gap ${selector} >`)) uncovered.push(`${file}: ${selector}`);
      }
    }
  }
  assert.deepEqual(uncovered, [], "New flex gap requires an old-WebKit spacing rule");
});

test("every focus-visible control keeps its indicator or feedback on old WebKit", () => {
  const uncovered = [];
  for (const file of cssFiles(join(root, "src"))) {
    const css = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const [, selectorList] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      for (const selector of selectorList.trim().split(/\s*,\s*/)) {
        if (selector.includes(":focus-visible") && !compat.includes(`.no-focus-visible ${selector.replace(":focus-visible", ":focus")}`)) uncovered.push(selector);
      }
    }
  }
  assert.deepEqual(uncovered, []);
});

test("production transforms dependency syntax and overlay geometry for Safari 13.1", async () => {
  const config = await resolveConfig({ root }, "build");
  assert.equal(config.build.target, "safari13.1");
  assert.equal(config.build.cssTarget, "safari13.1");
  const entry = "\0compatibility-regression";
  const result = await build({ root, logLevel: "silent", plugins: [{
    name: "compatibility-regression",
    resolveId(id) { if (id === "compatibility-regression") return entry; },
    load(id) {
      if (id !== entry) return;
      return `import ${JSON.stringify(join(root, "src/App.css"))};
        import ${JSON.stringify(join(root, "src/theme/compat.css"))};
        import ${JSON.stringify(join(root, "src/chrome/TrafficLights.css"))};
        class Counter { #value = 1; next() { return ++this.#value; } }
        const box = {}; box.counter ??= new Counter();
        globalThis.compatibilityResult = box.counter.next();`;
    },
  }], build: { write: false, minify: false, cssMinify: true, rollupOptions: { input: "compatibility-regression" } } });
  const output = (Array.isArray(result) ? result[0] : result).output;
  const javascript = output.find(item => item.type === "chunk").code;
  assert.doesNotMatch(javascript, /#value|\?\?=/);
  const context = {};
  runInNewContext(javascript, context);
  assert.equal(context.compatibilityResult, 2);
  const css = output.filter(item => item.type === "asset" && item.fileName.endsWith(".css")).map(item => String(item.source)).join("\n");
  assert.doesNotMatch(css, /[;{]\s*inset\s*:/, "old WebKit must get explicit overlay edges");
  assert.match(css, /\.visualizer-pane\{[^}]*top:0/);
  assert.match(css, /\.file-drop-target\{[^}]*top:6px/);
  assert.match(css, /\.traffic-light\{[^}]*border:1px solid var\(--edge\)/);
  assert.match(css, /@supports\s*\(color:color-mix/);
});
