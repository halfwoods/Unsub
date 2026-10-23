import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { beforeEach, test } from "node:test";

const helperUrl = new URL("../addMusic.ts", import.meta.url).href;
const modules = {
  "@tauri-apps/plugin-dialog": "export const open = (options) => globalThis.__unsubFolderTest.open(options);",
  "../api/commands": `
    export const libraryAddPaths = (paths) => globalThis.__unsubFolderTest.add(paths);
    export const welcomeDone = () => globalThis.__unsubFolderTest.completeWelcome();
    export const welcomeDue = () => globalThis.__unsubFolderTest.welcomeDue();
  `,
  "../state/store": `
    export const useApp = {
      getState: () => ({
        ...globalThis.__unsubFolderTest.app,
        showNotice: (message) => globalThis.__unsubFolderTest.notices.push(message),
      }),
      setState: (update) => {
        const state = globalThis.__unsubFolderTest;
        Object.assign(state.app, typeof update === "function" ? update(state.app) : update);
      },
    };
  `,
};

// Exercise the real workflow while substituting the native dialog and command boundary.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL === helperUrl && modules[specifier]) {
      return { url: `data:text/javascript,${encodeURIComponent(modules[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});

const { addMusicPaths, chooseMusicFolders, completeMusicWelcome, watchWelcomeDue } = await import(helperUrl);

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  globalThis.__unsubFolderTest = {
    notices: [],
    calls: [],
    picked: null,
    app: { welcoming: true, welcomeTipPending: false },
    welcomed: 0,
    welcomeDue: async () => true,
    completeWelcome: async () => { globalThis.__unsubFolderTest.welcomed += 1; },
    open: async (options) => {
      const state = globalThis.__unsubFolderTest;
      state.dialogOptions = options;
      return state.picked;
    },
    add: async (paths) => globalThis.__unsubFolderTest.calls.push(paths),
  };
});

test("a single chosen folder is added instead of an assumed Music directory", async () => {
  const state = globalThis.__unsubFolderTest;
  state.picked = "/Volumes/Records/My own music";

  assert.equal(await chooseMusicFolders(), true);
  assert.deepEqual(state.calls, [["/Volumes/Records/My own music"]]);
  assert.equal(state.dialogOptions.directory, true);
  assert.equal(state.dialogOptions.multiple, true);
  assert.deepEqual(state.notices, []);
});

test("multiple chosen folders are preserved, including spaces and non-ASCII names", async () => {
  const state = globalThis.__unsubFolderTest;
  state.picked = ["/Volumes/外付け/音楽", "/Users/person/Other music"];

  assert.equal(await chooseMusicFolders(), true);
  assert.deepEqual(state.calls, [state.picked]);
});

test("cancelling the folder chooser adds nothing and reports no error", async () => {
  const state = globalThis.__unsubFolderTest;
  assert.equal(await chooseMusicFolders(), false);
  assert.deepEqual(state.calls, []);
  assert.deepEqual(state.notices, []);
  assert.equal(state.app.welcoming, true);
  assert.equal(state.welcomed, 0);
});

test("a failed native folder chooser reports the error without adding anything", async () => {
  const state = globalThis.__unsubFolderTest;
  state.open = async () => { throw new Error("permission denied"); };

  assert.equal(await chooseMusicFolders(), false);
  assert.deepEqual(state.calls, []);
  assert.match(state.notices[0], /folder chooser.*permission denied/);
});

test("an add request failure is reported and the next attempt can succeed", async () => {
  const state = globalThis.__unsubFolderTest;
  state.picked = "/Volumes/Records";
  state.add = async () => { throw new Error("library is unavailable"); };

  assert.equal(await chooseMusicFolders(), false);
  assert.match(state.notices[0], /Couldn't add music.*library is unavailable/);

  state.add = async (paths) => state.calls.push(paths);
  assert.equal(await chooseMusicFolders(), true);
  assert.deepEqual(state.calls, [["/Volumes/Records"]]);
});

test("empty path lists never submit an add request", async () => {
  const state = globalThis.__unsubFolderTest;
  assert.equal(await addMusicPaths([]), false);
  assert.deepEqual(state.calls, []);
});

test("an accepted music choice reveals the library before its preference write finishes", async () => {
  const state = globalThis.__unsubFolderTest;
  let finish;
  state.completeWelcome = () => new Promise((resolve) => { finish = resolve; });

  const completing = completeMusicWelcome();
  assert.equal(state.app.welcoming, false);
  assert.equal(state.app.welcomeTipPending, true);
  finish();
  await completing;
});

test("Skip and Done dismiss immediately without scheduling import tips", async () => {
  const state = globalThis.__unsubFolderTest;
  let finish;
  state.completeWelcome = () => new Promise((resolve) => { finish = resolve; });

  const completing = completeMusicWelcome(false);
  assert.equal(state.app.welcoming, false);
  assert.equal(state.app.welcomeTipPending, false);
  finish();
  await completing;
});

test("a welcome preference failure does not hide an accepted native import", async () => {
  const state = globalThis.__unsubFolderTest;
  state.completeWelcome = async () => { throw new Error("disk full"); };

  await completeMusicWelcome();

  assert.equal(state.app.welcoming, false);
  assert.equal(state.app.welcomeTipPending, true);
  assert.match(state.notices[0], /welcome choice.*disk full/);
});

test("a startup response cannot reopen Welcome after an accepted music import", async () => {
  const state = globalThis.__unsubFolderTest;
  let finishStartup;
  state.app.welcoming = false;
  state.welcomeDue = () => new Promise((resolve) => { finishStartup = resolve; });
  const stop = watchWelcomeDue();

  await completeMusicWelcome();
  finishStartup(true);
  await settle();
  assert.equal(state.app.welcoming, false);
  stop();
});

test("a disposed startup check cannot reopen Welcome during a remount", async () => {
  const state = globalThis.__unsubFolderTest;
  let finishStartup;
  state.app.welcoming = false;
  state.welcomeDue = () => new Promise((resolve) => { finishStartup = resolve; });
  const stop = watchWelcomeDue();

  stop();
  finishStartup(true);
  await settle();
  assert.equal(state.app.welcoming, false);
});

test("startup opens Welcome when due, and reports query errors without an unhandled rejection", async () => {
  const state = globalThis.__unsubFolderTest;
  state.app.welcoming = false;
  const stop = watchWelcomeDue();
  await settle();
  assert.equal(state.app.welcoming, true);
  stop();

  state.welcomeDue = async () => { throw new Error("library unavailable"); };
  const stopFailed = watchWelcomeDue();
  await settle();
  assert.match(state.notices[0], /welcome choice.*library unavailable/);
  stopFailed();
});
