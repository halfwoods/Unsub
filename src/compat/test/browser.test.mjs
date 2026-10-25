import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../browser.ts", import.meta.url), "utf8");
const { supportsFlexGap, installBrowserCompatibility } = await import(`data:text/javascript,${encodeURIComponent(ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText)}`);

function environment({ flexGap = false, css = false, queueMicrotask } = {}) {
  const classes = new Set(), probes = [], timers = [];
  const root = { classList: { toggle(name, enabled) { if (enabled) classes.add(name); else classes.delete(name); } },
    appendChild(node) { probes.push(node); node.parent = root; } };
  const document = { documentElement: root, body: root,
    createElement() { return { style: {}, children: [], parent: null,
      appendChild(child) { this.children.push(child); },
      get scrollHeight() { assert.equal(this.children.length, 2); return flexGap ? 3 : 2; },
      remove() { this.parent = null; },
    }; },
  };
  return { document, CSS: { supports: () => css }, queueMicrotask, classes, probes, timers,
    setTimeout(callback) { timers.push(callback); },
  };
}

test("flex-gap probe measures layout instead of accepting grid-gap support, and cleans up", () => {
  for (const flexGap of [false, true]) {
    const host = environment({ flexGap, css: true });
    assert.equal(supportsFlexGap(host.document), flexGap);
    assert.equal(host.probes.length, 1);
    assert.equal(host.probes[0].parent, null);
  }
});

test("probe cleans up even when measuring layout fails", () => {
  const host = environment();
  host.document.body.appendChild = node => {
    host.probes.push(node); node.parent = host.document.body;
    Object.defineProperty(node, "scrollHeight", { get() { throw Error("layout failure"); } });
  };
  assert.throws(() => supportsFlexGap(host.document), /layout failure/);
  assert.equal(host.probes[0].parent, null);
});

test("only absent capabilities receive classes; native microtask API is preserved", () => {
  const native = () => {};
  const host = environment({ flexGap: true, css: true, queueMicrotask: native });
  installBrowserCompatibility(host);
  assert.deepEqual([...host.classes], []);
  assert.equal(host.queueMicrotask, native);
  host.CSS.supports = feature => feature !== "selector(:focus-visible)";
  installBrowserCompatibility(host);
  assert.deepEqual([...host.classes], ["no-focus-visible"]);
});

test("old WebKit gets complete fallback classes even when CSS.supports is unavailable", () => {
  const host = environment();
  delete host.CSS;
  installBrowserCompatibility(host);
  assert.deepEqual([...host.classes].sort(), ["no-aspect-ratio", "no-container-units", "no-flex-gap", "no-focus-visible"]);
});

test("microtask fallback is asynchronous and preserves Promise job FIFO order", async () => {
  const host = environment();
  installBrowserCompatibility(host);
  const events = [];
  host.queueMicrotask(() => { events.push("first"); host.queueMicrotask(() => events.push("nested")); });
  Promise.resolve().then(() => events.push("promise"));
  host.queueMicrotask(() => events.push("second"));
  assert.deepEqual(events, []);
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(events, ["first", "promise", "second", "nested"]);
  assert.equal(host.timers.length, 0);
  assert.throws(() => host.queueMicrotask(null), TypeError);
});

test("microtask exceptions report an error without rejecting a Promise or blocking later work", async () => {
  const host = environment(), error = Error("channel callback"), events = [];
  installBrowserCompatibility(host);
  host.queueMicrotask(() => { throw error; });
  host.queueMicrotask(() => events.push("continued"));
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(events, ["continued"]);
  assert.equal(host.timers.length, 1);
  assert.throws(host.timers[0], value => value === error);
});
