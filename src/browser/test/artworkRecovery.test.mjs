import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";
const component = new URL("../Artwork.tsx", import.meta.url).href;
const data = source => `data:text/javascript,${encodeURIComponent(source)}`;
const mocks = {
  react: "export const useState = () => [globalThis.failedSource, value => { globalThis.failedSource = value; }]; export const useEffect = (effect, deps) => { if(globalThis.lastSource !== deps[0]) { globalThis.lastSource = deps[0]; globalThis.pending = effect; } };",
  "react/jsx-runtime": "export const jsx = (type, props) => ({type, props}); export const jsxs = jsx;",
  "../api/commands": "export const artworkUrl = (id, source, size) => `artwork://${id}/${size}`;",
  "./Artwork.css": "export {};",
};
registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL === component && specifier in mocks) return { shortCircuit: true, url: data(mocks[specifier]) };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === component) return { shortCircuit: true, format: "module", source: ts.transpileModule(readFileSync(new URL(url), "utf8"), {
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
    }).outputText };
    return next(url, context);
  },
});
const { Artwork } = await import(component);
test("failed artwork retries a changed track or size and ignores stale errors", () => {
  globalThis.failedSource = null;
  const render = (trackId, size = 140) => { const node = Artwork({ trackId, size }); globalThis.pending?.(); globalThis.pending = null; return node; };
  const first = render(1); first.props.onError();
  assert.equal(render(1).type, "div");
  assert.equal(render(2).props.src, "artwork://2/140");
  first.props.onError();
  assert.equal(render(2).type, "img");
  const second = render(2); second.props.onError();
  assert.equal(render(2).type, "div");
  assert.equal(render(2, 200).props.src, "artwork://2/200");
  assert.equal(render(1).type, "img");
});
