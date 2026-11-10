import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";

const componentUrl = new URL("../../info/GetInfo.tsx", import.meta.url).href;
const data = source => `data:text/javascript,${encodeURIComponent(source)}`;
const mocks = {
  react: `export const useState = initial => globalThis.__info.useState(initial);
    export const useEffect = effect => effect(); export const useMemo = compute => compute();`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({type, props}); export const jsxs = jsx; export const Fragment = "fragment";`,
  "@tauri-apps/plugin-dialog": `export const open = async () => null;`,
  "../api/commands": `export const libraryEditTracks = async (...args) => { globalThis.__info.edits.push(args); return {}; };
    export const libraryLyrics = async () => null; export const libraryTrack = async () => null;`,
  "../browser/Artwork": `export const Artwork = () => null;`,
  "../state/store": `export const useApp = selector => selector(globalThis.__info.app);`,
  "../util/format": `export const dbOnlyNotice = () => null; export const formatBytes = String; export const formatClock = String;`,
  "./GetInfo.css": "",
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
const { GetInfo } = await import(componentUrl);

function* nodes(node) {
  if (Array.isArray(node)) { for (const child of node) yield* nodes(child); }
  else if (node && typeof node === "object") { yield node; yield* nodes(node.props?.children); }
}
function mount(t, tracks) {
  const hooks = new Map();
  let current, cursor;
  const h = {
    edits: [],
    app: { tracks, openLibrarySheet() {}, showNotice(message) { throw Error(message); } },
    useState(initial) {
      const values = hooks.get(current), index = cursor++;
      if (!(index in values)) values[index] = typeof initial === "function" ? initial() : initial;
      return [values[index], value => { values[index] = typeof value === "function" ? value(values[index]) : value; }];
    },
    component(type, props) {
      current = type; cursor = 0;
      if (!hooks.has(type)) hooks.set(type, []);
      return type(props);
    },
    render() {
      const props = { trackIds: tracks.map(track => track.id) };
      const sheet = h.component(GetInfo, props) ?? h.component(GetInfo, props);
      return h.component(sheet.type, sheet.props);
    },
    find(predicate) { return [...nodes(h.render())].find(predicate); },
    field(id) { return h.find(node => node.props.id === `info-${id}`); },
    checkbox(label) { return h.find(node => node.props["aria-label"] === `Change ${label} for all`); },
    change(id, value) { h.field(id).props.onChange({ target: { value } }); },
    async save() {
      h.find(node => node.type === "button" && node.props.children === "OK").props.onClick();
      await new Promise(setImmediate);
    },
  };
  globalThis.__info = h;
  t.after(() => { delete globalThis.__info; });
  h.render();
  h.find(node => node.props.role === "tab" && node.props.children === "Info").props.onClick();
  return h;
}
const song = (id, fields = {}) => ({ id, title: `Song ${id}`, artist: "Artist", albumArtist: "", album: "Album", genre: "Jazz", composer: "Composer",
  grouping: "Evening", comment: "First\nSecond", bpm: 120, year: 1999, durationMs: 1000, fileSize: 1000, dateModified: 1, dateAdded: 1, ...fields });

test("Info pairs metadata with its numbers and uses a three-row Comments control", t => {
  const h = mount(t, [song(1)]);
  const rows = [...nodes(h.render())].filter(node => String(node.props.className).startsWith("info-row"));
  assert.deepEqual(rows.map(row => [...nodes(row)].filter(node => node.props.id?.startsWith("info-")).map(node => node.props.id)), [
    ["info-title"], ["info-artist", "info-year"], ["info-albumArtist", "info-trackNumber"], ["info-album", "info-discNumber"],
    ["info-grouping", "info-bpm"], ["info-composer"], ["info-comment"], ["info-genre"],
  ]);
  assert.equal(h.field("comment").type, "textarea");
  assert.equal(h.field("comment").props.rows, 3);
  assert.equal(h.field("comment").props.value, "First\nSecond");
  assert.equal(h.field("bpm").props.inputMode, "numeric");
});

test("mixed extended fields stay untouched until selected and explicit blanks clear all songs", async t => {
  const h = mount(t, [song(1), song(2, { grouping: "Morning", comment: "Other", bpm: 90 })]);
  for (const [id, label] of [["grouping", "Grouping"], ["comment", "Comments"], ["bpm", "BPM"]]) {
    assert.equal(h.field(id).props.placeholder, "Mixed");
    assert.equal(h.checkbox(label).props.checked, false);
  }
  h.change("genre", "Rock");
  await h.save();
  assert.deepEqual(h.edits[0][1], { genre: "Rock" });
  for (const id of ["grouping", "comment", "bpm"]) h.change(id, "");
  await h.save();
  assert.deepEqual(h.edits[1][1], { genre: "Rock", grouping: "", comment: "", bpm: null });
});

test("shared values can be applied or deselected and comments retain line breaks", async t => {
  const h = mount(t, [song(1), song(2)]);
  assert.equal(h.field("bpm").props.value, "120");
  h.checkbox("BPM").props.onChange({ target: { checked: true } });
  h.checkbox("Grouping").props.onChange({ target: { checked: true } });
  h.checkbox("Grouping").props.onChange({ target: { checked: false } });
  h.change("comment", "New first\nNew second\nNew third");
  await h.save();
  assert.deepEqual(h.edits[0][1], { bpm: 120, comment: "New first\nNew second\nNew third" });
});
