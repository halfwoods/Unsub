import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";

const componentUrl = new URL("../ColumnHeadings.tsx", import.meta.url).href;
const data = source => `data:text/javascript,${encodeURIComponent(source)}`;
const mocks = {
  react: `export const useRef = value => globalThis.__headings.useRef(value); export const useState = value => globalThis.__headings.useState(value);`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({type, props}); export const jsxs = jsx; export const Fragment = "fragment";`,
  "../preferences/libraryViewStore": `export const useLibraryViews = { getState: () => globalThis.__headings.actions };`,
  "../preferences/libraryViews": `export const columnWidth = (_id, width) => width;`,
  "./columns": `export const ALL_COLUMNS = ["name", "time", "artist"].map(id => ({id})); export const columnLabel = id => id;`,
  "./columnFormat": `export const formatColumnText = () => "";`,
  "../state/store": `export const useApp = { getState: () => ({showNotice: message => { throw Error(message); }}) };`,
  "./songMenu": `export const SEPARATOR = {item:"Separator"}; export const showMenu = async (items, at) => globalThis.__headings.menus.push({items, at});`,
};
registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL === componentUrl && specifier in mocks) return { shortCircuit: true, url: data(mocks[specifier]) };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === componentUrl) return { shortCircuit: true, format: "module", source: ts.transpileModule(readFileSync(new URL(url), "utf8"), {
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
    }).outputText };
    return next(url, context);
  },
});
const { ColumnHeadings } = await import(componentUrl);

function mount(t, editable = true) {
  const hooks = [], header = {}, elements = new Map();
  let cursor = 0;
  const h = {
    updates: [], sorts: [], menus: [], hit: null,
    actions: { updateLayout: (...args) => h.updates.push(args) },
    useRef(value) { const index = cursor++; return hooks[index] ??= { current: value }; },
    useState(value) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = value;
      return [hooks[index], value => { hooks[index] = typeof value === "function" ? value(hooks[index]) : value; }];
    },
    props: {
      columns: ["name", "time", "artist"].map(id => ({id, sort: id, title: id, width: 100})),
      layout: { order: ["name", "time", "artist"], visible: ["name", "time", "artist"], widths: {} },
      source: "music", regular: false, editable, sort: "name", descending: false, tracks: [],
      preview() {}, sortBy: sort => h.sorts.push(sort),
    },
    render() { cursor = 0; return ColumnHeadings(h.props); },
    heading(id) { return h.render().props.children[1].find(child => child.props["data-column-id"] === id); },
    button(id) { return h.heading(id).props.children[0]; },
    element(id) {
      if (!elements.has(id)) {
        const captured = new Set();
        const element = { parentElement: header, dataset: {columnId: id}, captured,
          closest: () => element, setPointerCapture: id => captured.add(id), hasPointerCapture: id => captured.has(id), releasePointerCapture: id => captured.delete(id) };
        elements.set(id, element);
      }
      return elements.get(id);
    },
    event(id, patch = {}) { const element = h.element(id); return { button: 0, isPrimary: true, pointerId: 1, clientX: 10, clientY: 10,
      currentTarget: element, target: element, preventDefault() {}, stopPropagation() {}, ...patch }; },
    down(id, patch) { h.heading(id).props.onPointerDown(h.event(id, patch)); },
    move(id, target, patch = {}) { h.hit = target; h.heading(id).props.onPointerMove(h.event(id, {clientX: 110, ...patch})); },
    up(id, patch = {}) { h.heading(id).props.onPointerUp(h.event(id, {clientX: 110, ...patch})); },
  };
  globalThis.__headings = h;
  const previousDocument = globalThis.document;
  globalThis.document = { elementFromPoint: () => h.hit };
  t.after(() => { globalThis.document = previousDocument; });
  return h;
}

test("heading pointer drag reorders within the same header and suppresses its sort click", t => {
  const h = mount(t);
  h.down("name"); assert.equal(h.element("name").captured.has(1), true);
  h.move("name", h.element("artist")); h.up("name");
  assert.deepEqual(h.updates, [["music", {order: ["time", "artist", "name"]}, false]]);
  assert.equal(h.element("name").captured.size, 0);
  h.button("name").props.onClick({detail: 1}); assert.deepEqual(h.sorts, []);
  h.down("name"); h.up("name", {clientX: 10}); h.button("name").props.onClick({detail: 1});
  assert.deepEqual(h.sorts, ["name"]);
});

test("small pointer movement retains ordinary heading sorting", t => {
  const h = mount(t);
  h.down("name"); h.move("name", h.element("artist"), {clientX: 13, clientY: 12}); h.up("name");
  h.button("name").props.onClick({detail: 1});
  assert.deepEqual(h.updates, []); assert.deepEqual(h.sorts, ["name"]);
});

test("heading drag ignores secondary pointers and noneditable sources", t => {
  const h = mount(t);
  for (const patch of [{button: 2}, {isPrimary: false}]) {
    h.down("name", patch); h.move("name", h.element("artist")); h.up("name");
  }
  h.props.editable = false; h.down("name"); h.move("name", h.element("artist")); h.up("name");
  assert.deepEqual(h.updates, []); assert.equal(h.element("name").captured.size, 0);
});

test("cancelled capture and pointer loss never commit a heading reorder", t => {
  const h = mount(t);
  for (const handler of ["onPointerCancel", "onLostPointerCapture"]) {
    h.down("name"); h.move("name", h.element("artist"));
    h.heading("name").props[handler](h.event("name")); h.up("name");
    assert.equal(h.element("name").captured.size, 0);
  }
  assert.deepEqual(h.updates, []);
});

test("heading drops outside their header or after a source switch do not reorder", t => {
  const h = mount(t);
  const foreign = { parentElement: {}, dataset: {columnId: "artist"}, closest() { return this; } };
  h.down("name"); h.move("name", foreign); h.up("name");
  h.down("name"); h.move("name", h.element("artist")); h.props.source = "playlist:42"; h.up("name");
  assert.deepEqual(h.updates, []);
});

test("heading menus use click coordinates and keyboard heading bounds", t => {
  const h = mount(t);
  h.heading("name").props.onContextMenu(h.event("name", {clientX: 62, clientY: 91}));
  h.button("name").props.onKeyDown({ key: "F10", shiftKey: true, preventDefault() {}, stopPropagation() {},
    currentTarget: {getBoundingClientRect: () => ({left: 43, bottom: 105})} });
  assert.deepEqual(h.menus.map(({at}) => [at.x, at.y]), [[62, 91], [43, 105]]);
});
