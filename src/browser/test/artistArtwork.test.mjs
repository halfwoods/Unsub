import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { test } from "node:test";
import ts from "typescript";

const component = new URL("../ArtistArtwork.tsx", import.meta.url).href;
const data = source => `data:text/javascript,${encodeURIComponent(source)}`;
const mocks = {
  react: "export const useState = () => [globalThis.__artworkFailed, update => { globalThis.__artworkFailed = update(globalThis.__artworkFailed); }];",
  "react/jsx-runtime": "export const jsx = (type, props) => ({type, props}); export const jsxs = jsx;",
  "../api/commands": "export const artworkUrl = id => `artwork://${id}`;",
  "./browseHelpers": "export const artistInitials = () => 'AR';",
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
const { ArtistArtwork } = await import(component);

test("mosaic geometry follows remaining covers without :has, including failed artwork", () => {
  globalThis.__artworkFailed = new Set();
  const render = () => ArtistArtwork({ name: "Artist", trackIds: [1, 2, 3], size: 120 });
  let tile = render();
  assert.match(tile.props.className, /artist-mosaic--multiple/);
  assert.doesNotMatch(tile.props.className, /artist-mosaic--two/);
  tile.props.children[0][0].props.onError();
  tile = render();
  assert.match(tile.props.className, /artist-mosaic--two/);
  tile.props.children[0][0].props.onError();
  tile = render();
  assert.doesNotMatch(tile.props.className, /artist-mosaic--multiple|artist-mosaic--two/);
  tile.props.children[0][0].props.onError();
  assert.equal(render().props.children[0].props.children, "AR");
});
