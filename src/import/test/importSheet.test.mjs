import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { beforeEach, test } from "node:test";

const storeUrl = new URL("../../state/store.ts", import.meta.url).href;
const modules = {
  "../api/commands": "export const importMusicInspect = (xml) => globalThis.__unsubImportSheetTest.inspect(xml);",
  "../preferences/playbackPrefs": "export const loadVisualizerPrefs = () => ({}); export const saveVisualizerPrefs = () => {};",
  "../preferences/textSizes": `
    export const loadTextSizes = () => ({});
    export const listRowHeight = () => 20;
    export const applyTextSizes = () => {};
    export const saveTextSizes = () => {};
  `,
};

// Keep the real Zustand actions and substitute only native inspection and local preferences.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL === storeUrl && modules[specifier]) {
      return { url: `data:text/javascript,${encodeURIComponent(modules[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});

const { useApp } = await import(storeUrl);

beforeEach(() => {
  globalThis.__unsubImportSheetTest = { notices: [] };
  useApp.setState({
    importSheet: null,
    welcoming: true,
    showNotice: (message) => globalThis.__unsubImportSheetTest.notices.push(message),
  });
});

test("failed Music-library inspection reports failure to Welcome and allows retry", async () => {
  const state = globalThis.__unsubImportSheetTest;
  state.inspect = async () => { throw new Error("Media & Apple Music permission denied"); };

  assert.equal(await useApp.getState().openImportSheet(null), false);
  assert.equal(useApp.getState().importSheet, null);
  assert.match(state.notices[0], /permission denied/);

  const preview = { tracks: 3 };
  state.inspect = async (xml) => {
    assert.equal(xml, null);
    return preview;
  };
  assert.equal(await useApp.getState().openImportSheet(null), true);
  assert.equal(useApp.getState().importSheet, preview);
});

test("successful XML inspection passes the explicit path and opens its preview", async () => {
  const preview = { tracks: 2 };
  globalThis.__unsubImportSheetTest.inspect = async (xml) => {
    assert.equal(xml, "/Volumes/My Music/Library.xml");
    return preview;
  };

  assert.equal(await useApp.getState().openImportSheet("/Volumes/My Music/Library.xml"), true);
  assert.equal(useApp.getState().importSheet, preview);
});
