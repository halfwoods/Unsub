import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";
const component = new URL("../CoverFlow.tsx", import.meta.url).href;
const data = source => `data:text/javascript,${encodeURIComponent(source)}`;
const mocks = {
  react: "export const useState = initial => [globalThis.centerKey ?? initial, value => { globalThis.centerKey = value; }]; export const useMemo = fn => fn(); export const useEffect = effect => { globalThis.effect = effect; }; export const useRef = () => ({current:null});",
  "react/jsx-runtime": "export const jsx = (type, props) => ({type, props}); export const jsxs = jsx;",
  "../state/store": "export const useApp = fn => fn(globalThis.state); useApp.getState = () => globalThis.state;",
  "../util/platform": "export const isContextClick = () => false;",
  "./Artwork": "export const Artwork = () => null;",
  "./SongList": "export const SongList = () => null;",
  "./songMenu": "export const showAlbumMenu = () => {};",
  "./CoverFlow.css": "export {};",
};
registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL === component && specifier in mocks) return { shortCircuit: true, url: data(mocks[specifier]) };
    if (context.parentURL === component && specifier === "./albumKeys") return { shortCircuit:true,url:new URL("../albumKeys.ts",import.meta.url).href };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === component) return { shortCircuit: true, format: "module", source: ts.transpileModule(readFileSync(new URL(url), "utf8"), {
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
    }).outputText };
    return next(url, context);
  },
});
const { CoverFlow } = await import(component);
test("Cover Flow follows the active song, preserves multiselection and retains album identity after filtering", () => {
  const calls = [];
  globalThis.state = { albums:[{key:"a"},{key:"b"},{key:"c"}], tracks:[{id:1,albumGroup:"a"},{id:2,albumGroup:"b"},{id:3,albumGroup:"c"}], selection:new Set([1,3]), selectionAnchor:2, selectRow:(...args)=>calls.push(args) };
  const render = () => CoverFlow().props.children[0];
  render(); globalThis.effect();
  assert.equal(render().props.center,2);
  render().props.onCenter(2); assert.equal(calls.length,0);
  globalThis.state.albums = [{key:"c"},{key:"a"}];
  assert.equal(render().props.center,0);
  render().props.onCenter(1); assert.equal(calls.at(-1)[0],0);
  globalThis.state.albums = [];
  assert.equal(render().props.center,0);
});

test("Cover Flow renders at most 19 covers in a large library and prevents native arrow scrolling", () => {
  globalThis.centerKey = null;
  globalThis.state = { albums:Array.from({length:10000},(_,i)=>({key:String(i),album:String(i),artworkTrackId:i})), tracks:[], selection:new Set(), selectionAnchor:null, selectRow:()=>{} };
  const props = CoverFlow().props.children[0];
  const moves = [];
  const stage = props.type({...props.props,center:5000,onCenter:i=>moves.push(i)});
  assert.equal(stage.props.children[0].props.children.length,19);
  let prevented=false;
  stage.props.onKeyDown({key:"ArrowRight",preventDefault:()=>{prevented=true;}});
  assert.equal(prevented,true); assert.deepEqual(moves,[5001]);
});
