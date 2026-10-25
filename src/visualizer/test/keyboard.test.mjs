import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";

const component = new URL("../Visualizer.tsx", import.meta.url).href;
const data = source => `data:text/javascript,${encodeURIComponent(source)}`;
const mocks = {
  react: `export const useEffect = callback => globalThis.__visualizerEffects.push(callback);
    export const useRef = current => ({ current });
    export const useState = initial => [initial, update => globalThis.__visualizerUpdates.push(update)];`,
  "react/jsx-runtime": "export const jsx = (type, props) => ({type, props}); export const jsxs = jsx;",
  "./runtime": "export class VisualizerRuntime {}",
  "./Visualizer.css": "",
};
registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL === component) {
      if (specifier in mocks) return { shortCircuit: true, url: data(mocks[specifier]) };
      if (specifier.startsWith("./")) return { shortCircuit: true, url: new URL(`${specifier}.ts`, component).href };
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === component) return { shortCircuit: true, format: "module", source: ts.transpileModule(readFileSync(new URL(url), "utf8"), {
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
    }).outputText };
    return next(url, context);
  },
});
const { Visualizer } = await import(component);

function mounted(run) {
  const names = ["window", "document", "HTMLElement", "__visualizerEffects", "__visualizerUpdates"];
  const originals = names.map(name => Object.getOwnPropertyDescriptor(globalThis, name));
  const listeners = new Map();
  const events = [];
  let overlay = null;
  class Element {
    constructor(tagName = "DIV", isContentEditable = false) { Object.assign(this, { tagName, isContentEditable }); }
  }
  const globals = {
    window: {
      addEventListener: (name, handler) => listeners.set(name, handler),
      removeEventListener: name => listeners.delete(name),
    },
    document: { querySelector: () => overlay },
    HTMLElement: Element,
    __visualizerEffects: [],
    __visualizerUpdates: [],
  };
  for (const name of names) Object.defineProperty(globalThis, name, { configurable: true, value: globals[name] });
  const cleanups = [];
  try {
    Visualizer({ feed: null, nowPlaying: null, style: "gaslight",
      onStyleChange: style => events.push(style), onToggleFullscreen: () => events.push("fullscreen"), onClose: () => events.push("close"),
    });
    for (const effect of globals.__visualizerEffects) cleanups.push(effect());
    globals.__visualizerUpdates.length = 0;
    run({
      events, updates: globals.__visualizerUpdates, Element,
      overlay: role => { overlay = role ? { role } : null; },
      key: (key, changes = {}) => {
        let prevented = false;
        listeners.get("keydown")({ key, target: new Element(), preventDefault: () => { prevented = true; }, ...changes });
        return prevented;
      },
    });
  } finally {
    for (const cleanup of cleanups) cleanup?.();
    assert.equal(listeners.size, 0, "unmount releases the keyboard handler");
    names.forEach((name, index) => {
      if (originals[index]) Object.defineProperty(globalThis, name, originals[index]);
      else delete globalThis[name];
    });
  }
}

test("Preferences, alerts and menus retain all keyboard commands above the visualizer", () => {
  mounted(({ events, updates, overlay, key, Element }) => {
    for (const role of ["dialog", "alertdialog", "menu"]) {
      overlay(role);
      for (const target of [new Element("BUTTON"), new Element("DIV")]) {
        for (const command of ["ArrowLeft", "ArrowRight", "f", "F", "m", "M", "Escape"]) {
          assert.equal(key(command, { target }), false, `${role} keeps ${command}`);
        }
      }
    }
    assert.deepEqual(events, []);
    assert.deepEqual(updates, []);
  });
});

test("uncovered visualizer keeps style, fullscreen, info and close commands", () => {
  mounted(({ events, updates, key }) => {
    for (const command of ["ArrowLeft", "ArrowRight", "f", "m", "Escape"]) assert.equal(key(command), true);
    assert.deepEqual(events, ["classic", "lights", "fullscreen", "close"]);
    assert.equal(updates.length, 1);
    assert.equal(updates[0](false), true);
  });
});

test("editing and modified or already-handled keys do not control the visualizer", () => {
  mounted(({ events, updates, key, Element }) => {
    for (const target of [new Element("INPUT"), new Element("TEXTAREA"), new Element("SELECT"), new Element("DIV", true)]) {
      assert.equal(key("ArrowRight", { target }), false);
    }
    for (const property of ["metaKey", "ctrlKey", "altKey", "defaultPrevented"]) assert.equal(key("f", { [property]: true }), false);
    assert.deepEqual(events, []);
    assert.deepEqual(updates, []);
  });
});
