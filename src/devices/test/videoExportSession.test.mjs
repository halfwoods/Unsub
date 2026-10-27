import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createVideoExportSession } from '../videoExportSession.ts';
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const listing = { items: [{ id: 'a', protection: 'protected' }, { id: 'b', protection: 'unknown' }], fromFiles: false, warning: null };
const preview = { videos: 2, missing: 0, bytes: 100, freeBytes: 200, fits: true, folder: '/export', protected: 1, unknownProtection: 1 };
function make(overrides = {}) { return createVideoExportSession('device', { list: async () => listing, preview: async () => preview, start: async () => {}, ...overrides }); }

test('selects protected and unknown originals, rejects unknown IDs and empty selection', async () => {
  const requests = [];
  const session = make({ preview: async (...args) => { requests.push(args); return preview; } });
  await session.load();
  assert.deepEqual(session.getSnapshot().selected, ['a', 'b']);
  assert.equal(session.canStart(), false);
  session.destination('/export'); await tick();
  assert.equal(session.canStart(), true);
  session.select(['a', 'a', 'bogus']); await tick();
  assert.deepEqual(requests.at(-1), ['device', ['a'], '/export']);
  session.select([]);
  assert.equal(session.canStart(), false);
  assert.equal(session.getSnapshot().preview, null);
});
test('older previews and failures cannot overwrite newer destination or selection', async () => {
  const requests = [];
  const session = make({ preview: () => { const next = deferred(); requests.push(next); return next.promise; } });
  await session.load(); session.destination('/old'); session.destination('/new');
  requests[1].resolve({ ...preview, folder: '/new' }); await tick();
  requests[0].reject(new Error('old failure')); await tick();
  assert.equal(session.getSnapshot().preview.folder, '/new');
  assert.equal(session.getSnapshot().error, null);
  session.select(['b']);
  assert.equal(session.canStart(), false);
  assert.equal(session.getSnapshot().preview, null);
  session.select([]); requests[2].resolve(preview); await tick();
  assert.equal(session.getSnapshot().preview, null);
});
test('disposed sheet ignores pending work and StrictMode reload ignores first request', async () => {
  const first = deferred(), second = deferred(); let calls = 0;
  const session = make({ list: () => (++calls === 1 ? first : second).promise });
  const old = session.load(); session.dispose(); const current = session.load();
  second.resolve(listing); await current; first.resolve({ items: [] }); await old;
  assert.equal(session.getSnapshot().listing, listing);
  session.destination('/export'); session.dispose(); await tick();
  assert.equal(session.getSnapshot().preview, null);
  assert.equal(await session.start(), false);
});
test('start is single-flight and failures require a fresh destination preview', async () => {
  const pending = deferred(); let calls = 0;
  const session = make({ start: () => { calls++; return pending.promise; } });
  await session.load(); session.destination('/export'); await tick();
  const start = session.start();
  assert.equal(await session.start(), false);
  session.select([]); session.destination('/changed');
  assert.deepEqual(session.getSnapshot().selected, ['a', 'b']);
  assert.equal(session.getSnapshot().destination, '/export');
  pending.reject(new Error('device disconnected')); assert.equal(await start, false);
  assert.equal(calls, 1); assert.equal(session.canStart(), false);
  assert.match(session.getSnapshot().error, /disconnected/);
  session.destination('/export'); await tick(); assert.equal(session.canStart(), true);
});
test('unavailable-only and insufficient-space previews cannot start', async () => {
  for (const result of [{ ...preview, missing: 2 }, { ...preview, fits: false }]) {
    const session = make({ preview: async () => result });
    await session.load(); session.destination('/export'); await tick();
    assert.equal(session.canStart(), false); assert.equal(await session.start(), false);
  }
});
test('list failure can be retried and preview errors leave controls usable', async () => {
  let calls = 0;
  const session = make({ list: async () => { if (!calls++) throw new Error('unplugged'); return listing; }, preview: async () => { throw new Error('not writable'); } });
  await session.load(); assert.equal(session.getSnapshot().loading, false);
  assert.match(session.getSnapshot().error, /unplugged/);
  await session.load(); assert.equal(session.getSnapshot().error, null);
  session.destination('/export'); await tick();
  assert.equal(session.getSnapshot().counting, false);
  assert.match(session.getSnapshot().error, /not writable/);
});
