import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";

// Exercise the actual component's input handlers and subscription without a DOM.
const componentUrl = new URL("../../chrome/SearchField.tsx", import.meta.url).href;
const data = (source) => `data:text/javascript,${encodeURIComponent(source)}`;
registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL === componentUrl) {
      if (specifier === "react") return { shortCircuit: true, url: data(`
        export const useState = value => globalThis.__searchField.useState(value);
        export const useRef = value => globalThis.__searchField.useRef(value);
        export const useEffect = (effect, deps) => globalThis.__searchField.useEffect(effect, deps);
      `) };
      if (specifier === "react/jsx-runtime") return { shortCircuit: true, url: data("export const jsx = (type, props) => ({ type, props }); export const jsxs = jsx;") };
      if (specifier === "../state/store") return { shortCircuit: true, url: data(`
        export const useApp = select => select(globalThis.__searchField.state);
        useApp.getState = () => globalThis.__searchField.state;
        useApp.subscribe = listener => globalThis.__searchField.subscribe(listener);
      `) };
      if (specifier.endsWith(".css")) return { shortCircuit: true, url: data("export {};") };
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === componentUrl) return { shortCircuit: true, format: "module", source: ts.transpileModule(readFileSync(new URL(url), "utf8"), {
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
    }).outputText };
    return next(url, context);
  },
});
const { SearchField } = await import(componentUrl);

function mount(t, initial = {}) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const hooks = [], listeners = new Set(), cleanups = [];
  let cursor = 0, effects = [];
  const h = {
    calls: [],
    state: { source: "music", query: { search: null }, songReveal: null, ...initial },
    useState(value) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = typeof value === "function" ? value() : value;
      return [hooks[index], value => { hooks[index] = typeof value === "function" ? value(hooks[index]) : value; }];
    },
    useRef(value) { const index = cursor++; return hooks[index] ??= { current: value }; },
    useEffect(effect, deps) {
      const index = cursor++, previous = hooks[index];
      if (!previous || deps.some((value, i) => value !== previous[i])) { hooks[index] = deps; effects.push(effect); }
    },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    update(patch) { const previous = h.state; h.state = { ...previous, ...patch }; for (const listener of listeners) listener(h.state, previous); },
    render() { cursor = 0; const tree = SearchField(); const pending = effects; effects = []; for (const effect of pending) cleanups.push(effect()); return tree; },
    input() { return h.render().props.children.find(child => child?.type === "input"); },
    type(value) { h.input().props.onChange({ target: { value } }); },
    unmount() { for (const cleanup of cleanups.splice(0)) cleanup?.(); },
  };
  h.state.setSearch = value => {
    h.calls.push(value);
    const search = value.trim() || null;
    if (search !== h.state.query.search) h.update({ query: { ...h.state.query, search }, songReveal: null });
  };
  globalThis.__searchField = h;
  h.render();
  t.after(() => h.unmount());
  return h;
}

test("search debounces rapid typing and preserves whitespace for the next word", t => {
  const h = mount(t);
  h.type("Ann"); t.mock.timers.tick(100); h.type("Ann "); t.mock.timers.tick(149);
  assert.deepEqual(h.calls, []);
  t.mock.timers.tick(1);
  assert.deepEqual(h.calls, ["Ann "]); assert.equal(h.state.query.search, "Ann"); assert.equal(h.input().props.value, "Ann ");
  h.type("Ann live"); t.mock.timers.tick(150);
  assert.equal(h.state.query.search, "Ann live");
});

test("external search reset clears the visible text and cancels pending input", t => {
  const h = mount(t, { query: { search: "Annabelle" } });
  assert.equal(h.input().props.value, "Annabelle");
  h.type("Annabelle live");
  h.update({ query: { search: null } });
  assert.equal(h.input().props.value, "");
  t.mock.timers.tick(200); assert.deepEqual(h.calls, []); assert.equal(h.state.query.search, null);
});

test("revealing a visible song cancels pending input even when the query is unchanged", t => {
  const h = mount(t, { query: { search: "Ann" } });
  h.type("Annabelle");
  h.update({ songReveal: { trackId: 1, source: "music", query: h.state.query } });
  assert.equal(h.input().props.value, "Ann");
  t.mock.timers.tick(200); assert.deepEqual(h.calls, []); assert.equal(h.state.query.search, "Ann");
});

test("source changes and unmount both cancel delayed search work", t => {
  const h = mount(t);
  h.type("pending"); h.update({ source: "playlist:42", query: { search: null } });
  assert.equal(h.input().props.value, "");
  t.mock.timers.tick(200); assert.deepEqual(h.calls, []);
  h.type("another"); h.unmount(); t.mock.timers.tick(200); assert.deepEqual(h.calls, []);
});

test("unrelated query changes preserve pending text, and Escape clears it", t => {
  const h = mount(t);
  h.type("pending"); h.update({ query: { ...h.state.query, sort: "year" } });
  assert.equal(h.input().props.value, "pending");
  t.mock.timers.tick(150); assert.equal(h.state.query.search, "pending");
  h.input().props.onKeyDown({ key: "Escape" });
  t.mock.timers.tick(150); assert.equal(h.state.query.search, null); assert.equal(h.input().props.value, "");
});
