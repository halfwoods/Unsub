import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createVideoAddSession, MAX_VIDEO_ADD_FILES } from '../videoAddSession.ts';

const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const item = (path, overrides = {}) => ({ path, title: path, bytes: 10, durationMs: 1000, reason: null, alreadyPresent: false, ...overrides });
const result = (paths = [], overrides = {}) => ({ items: paths.map(path => item(path)), bytes: paths.length * 10, freeBytes: 10000, fits: true, supported: true, warning: null, ...overrides });
function make(overrides = {}) { return createVideoAddSession('ipod', { preview: async (_device, paths) => result(paths), cancelPreview: async () => {}, start: async () => {}, ...overrides }); }

test('empty selection checks device capability, and only accepted unique selected paths are submitted', async () => {
  const previews = [], starts = [];
  const session = make({
    preview: async (...args) => {
      previews.push(args);
      return result(args[1], { items: [item('/good.mp4'), item('/good.mp4'), item('/bad.mp4', { reason: 'H.264 High profile is unsupported' }), item('/known.mp4', { alreadyPresent: true }), item('/not-selected.mp4')] });
    },
    start: async (...args) => { starts.push(args); },
  });
  await session.load();
  assert.deepEqual(previews[0].slice(0, 3), ['ipod', [], 'movie']);
  assert.equal(typeof previews[0][3], 'string');
  assert.equal(session.canStart(), false);
  session.select(['/good.mp4', '/bad.mp4', '/known.mp4', '/good.mp4', '']); await tick();
  assert.deepEqual(session.getSnapshot().paths, ['/good.mp4', '/bad.mp4', '/known.mp4']);
  assert.deepEqual(session.acceptedPaths(), ['/good.mp4']);
  assert.equal(session.canStart(), true);
  assert.equal(await session.start(), true);
  assert.deepEqual(starts, [['ipod', ['/good.mp4'], 'movie']]);
});

test('category and selection changes invalidate preview and discard older success or failure', async () => {
  const requests = [];
  const session = make({ preview: (_device, paths, kind) => { const request = deferred(); requests.push({ ...request, paths, kind }); return request.promise; } });
  const loaded = session.load(); requests[0].resolve(result()); await loaded;
  session.select(['/old.mp4']);
  session.select(['/new.mp4']);
  session.category('musicVideo');
  requests[3].resolve(result(['/new.mp4'])); await tick();
  requests[1].resolve(result(['/old.mp4'])); requests[2].reject(new Error('stale failure')); await tick();
  assert.deepEqual(session.acceptedPaths(), ['/new.mp4']);
  assert.equal(session.getSnapshot().kind, 'musicVideo');
  assert.equal(session.getSnapshot().error, null);
  assert.equal(session.canStart(), true);
  session.select([]);
  assert.equal(session.getSnapshot().preview, null);
  assert.equal(session.canStart(), false);
  requests[4].resolve(result()); await tick();
  assert.deepEqual(session.acceptedPaths(), []);
});

test('disposed sheets and StrictMode reload cannot accept stale preview results', async () => {
  const requests = [];
  const session = make({ preview: () => { const request = deferred(); requests.push(request); return request.promise; } });
  const first = session.load(); session.dispose(); const second = session.load();
  requests[1].resolve(result([], { supported: false, warning: 'This model cannot play videos.' })); await second;
  requests[0].resolve(result()); await first;
  assert.equal(session.getSnapshot().supported, false);
  session.select(['/x.mp4']); session.dispose(); requests[2].resolve(result(['/x.mp4'])); await tick();
  assert.equal(session.getSnapshot().preview, null);
  assert.equal(await session.start(), false);
  session.select(['/y.mp4']); session.category('musicVideo'); await session.retry();
  assert.equal(requests.length, 3);
  assert.deepEqual(session.getSnapshot().paths, ['/x.mp4']);
});

test('start is single-flight, freezes selected paths/category, and failure requires a fresh check', async () => {
  const pending = deferred(); let starts = 0;
  const session = make({ start: () => { starts++; return pending.promise; } });
  await session.load(); session.select(['/good.mp4']); await tick();
  session.category('musicVideo'); await tick();
  const start = session.start();
  assert.equal(await session.start(), false);
  session.select([]); session.category('movie'); await session.retry();
  assert.deepEqual(session.getSnapshot().paths, ['/good.mp4']);
  assert.equal(session.getSnapshot().kind, 'musicVideo');
  pending.reject(new Error('device disconnected')); assert.equal(await start, false);
  assert.equal(starts, 1);
  assert.equal(session.getSnapshot().preview, null);
  assert.equal(session.canStart(), false);
  assert.match(session.getSnapshot().error, /disconnected/);
  await session.retry();
  assert.equal(session.canStart(), true);
});

test('unsupported, insufficient-space, rejected-only and already-present-only selections cannot start', async () => {
  const results = [
    result(['/x'], { supported: false }), result(['/x'], { fits: false }),
    result(['/x'], { items: [item('/x', { reason: 'Invalid file' })] }),
    result(['/x'], { items: [item('/x', { alreadyPresent: true })] }),
  ];
  for (const preview of results) {
    const session = make({ preview: async () => preview });
    await session.load(); session.select(['/x']); await tick();
    assert.equal(session.canStart(), false);
    assert.equal(await session.start(), false);
  }
});

test('oversized file choices are rejected without changing selection or leaving an older preview startable', async () => {
  const pending = deferred(); let calls = 0;
  const session = make({ preview: (_device, paths) => ++calls === 1 ? Promise.resolve(result()) : pending.promise });
  await session.load(); session.select(['/good.mp4']);
  session.select(Array.from({ length: MAX_VIDEO_ADD_FILES + 1 }, (_, index) => `/video-${index}.mp4`));
  assert.equal(calls, 2);
  assert.deepEqual(session.getSnapshot().paths, ['/good.mp4']);
  assert.match(session.getSnapshot().error, /500/);
  assert.equal(session.getSnapshot().checking, false);
  pending.resolve(result(['/good.mp4'])); await tick();
  assert.equal(session.getSnapshot().preview, null);
  assert.equal(session.canStart(), false);
});

test('preview failures recover through retry without replacing the chosen files', async () => {
  let fail = true;
  const session = make({ preview: async (_device, paths) => { if (fail) throw new Error('temporarily unavailable'); return result(paths); } });
  await session.load();
  assert.equal(session.getSnapshot().checking, false);
  assert.match(session.getSnapshot().error, /unavailable/);
  fail = false; await session.retry();
  assert.equal(session.getSnapshot().supported, true);
  session.select(['/good.mp4']); await tick();
  fail = true; await session.retry();
  assert.equal(session.canStart(), false);
  assert.deepEqual(session.getSnapshot().paths, ['/good.mp4']);
  fail = false; await session.retry();
  assert.equal(session.getSnapshot().error, null);
  assert.equal(session.canStart(), true);
});

test('dismissal cancels only its pending request and every session/request gets a distinct identity', async () => {
  const requests = [], cancellations = [];
  const api = {
    preview: (_device, paths, _kind, requestId) => { const pending = deferred(); requests.push({ ...pending, paths, requestId }); return pending.promise; },
    cancelPreview: async requestId => { cancellations.push(requestId); },
  };
  const first = make(api), second = make(api);
  const firstLoad = first.load(); first.dispose();
  const repeatedLoad = first.load(); const secondLoad = second.load();
  assert.deepEqual(cancellations, [requests[0].requestId]);
  assert.equal(new Set(requests.map(request => request.requestId)).size, 3);
  requests[1].resolve(result()); await repeatedLoad;
  // Finishing an obsolete request cannot remove the identity of the newer one.
  requests[0].resolve(result([], { supported: false })); await firstLoad;
  second.dispose();
  assert.deepEqual(cancellations, [requests[0].requestId, requests[2].requestId]);
  requests[2].resolve(result()); await secondLoad;
  assert.equal(first.getSnapshot().supported, true);
  first.dispose();
  assert.equal(cancellations.length, 2, 'completed checks need no cancellation');
});

test('a stale request settling does not prevent cancellation of the latest check; cancellation failures are ignored', async () => {
  const requests = [], cancellations = [];
  const session = make({
    preview: (_device, _paths, _kind, requestId) => { const pending = deferred(); requests.push({ ...pending, requestId }); return pending.promise; },
    cancelPreview: async requestId => { cancellations.push(requestId); throw new Error('window closed'); },
  });
  const initial = session.load();
  session.select(['/latest.mp4']);
  requests[0].resolve(result()); await initial;
  session.dispose(); await tick();
  assert.deepEqual(cancellations, [requests[1].requestId]);
  requests[1].resolve(result(['/latest.mp4'])); await tick();
  assert.equal(session.getSnapshot().preview, null);
  assert.equal(session.canStart(), false);
});
