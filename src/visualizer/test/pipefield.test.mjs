import assert from "node:assert/strict";
import { test } from "node:test";

import {
  GRID_X,
  GRID_Y,
  GRID_Z,
  MAX_PIPES,
  PIECE_STRIDE,
  PipeField,
} from "../pipefield.ts";

const STEP = 1 / 60;

function run(field, seconds, speed = 4, hue = 0.3) {
  for (let i = 0; i < Math.round(seconds / STEP); i++) field.step(STEP, speed, hue);
}

/** Every finished cylinder as [ax, ay, az, bx, by, bz], in grid units. */
function segments(field) {
  const out = [];
  for (let i = 0; i < field.segmentCount; i++) {
    const s = field.segments.subarray(i * PIECE_STRIDE, (i + 1) * PIECE_STRIDE);
    out.push([
      s[0] + (GRID_X - 1) / 2,
      s[1] + (GRID_Y - 1) / 2,
      s[2] + (GRID_Z - 1) / 2,
      s[4] + (GRID_X - 1) / 2,
      s[5] + (GRID_Y - 1) / 2,
      s[6] + (GRID_Z - 1) / 2,
    ]);
  }
  return out;
}

test("pipes run one cell at a time along the grid and stay inside it", () => {
  const field = new PipeField(7);
  run(field, 30);
  assert.ok(field.segmentCount > 20, `${field.segmentCount} segments`);
  for (const [ax, ay, az, bx, by, bz] of segments(field)) {
    for (const [value, size] of [[ax, GRID_X], [ay, GRID_Y], [az, GRID_Z], [bx, GRID_X], [by, GRID_Y], [bz, GRID_Z]]) {
      assert.ok(Math.abs(value - Math.round(value)) < 1e-4, `${value} is on a node`);
      assert.ok(value > -1e-4 && value < size - 1 + 1e-4, `${value} is inside ${size}`);
    }
    assert.ok(Math.abs(Math.abs(bx - ax) + Math.abs(by - ay) + Math.abs(bz - az) - 1) < 1e-4, "one cell long, along an axis");
  }
});

test("no two pipes share a cell", () => {
  const field = new PipeField(3);
  run(field, 40, 6);
  const nodes = new Map();
  const key = (x, y, z) => `${Math.round(x)},${Math.round(y)},${Math.round(z)}`;
  for (const [ax, ay, az, bx, by, bz] of segments(field)) {
    for (const node of [key(ax, ay, az), key(bx, by, bz)]) nodes.set(node, (nodes.get(node) ?? 0) + 1);
  }
  // A node on a pipe is the end of one cylinder and the start of the next; never of a third.
  for (const [node, count] of nodes) assert.ok(count <= 2, `${node} is used ${count} times`);
});

test("a few pipes grow at once, each in a colour of its own", () => {
  const field = new PipeField(5);
  for (let beat = 0; beat < 8; beat++) field.beat();
  field.step(STEP, 3, 0.2);
  assert.equal(field.pipeCount, MAX_PIPES);
  const hues = new Set();
  for (let i = 0; i < field.headCount; i++) hues.add(field.heads[i * PIECE_STRIDE + 7].toFixed(3));
  assert.equal(hues.size, MAX_PIPES, "four different hues");
});

test("a beat starts a pipe, and once there are enough it makes one turn", () => {
  const field = new PipeField(11);
  const before = field.pipeCount;
  field.beat();
  assert.equal(field.pipeCount, before + 1);
  field.beat();
  field.beat();
  assert.equal(field.pipeCount, MAX_PIPES);

  // With the straight way always free in a long run, a forced turn is the only reason to turn.
  let forced = 0;
  let unforced = 0;
  for (let trial = 0; trial < 40; trial++) {
    for (const force of [false, true]) {
      const a = new PipeField(100 + trial);
      const joints = () => a.jointCount;
      run(a, 0.3, 2);
      const start = joints();
      if (force) for (let i = 0; i < 12; i++) a.beat();
      run(a, 0.5, 4);
      if (force) forced += joints() - start;
      else unforced += joints() - start;
    }
  }
  assert.ok(forced > unforced, `${forced} corners with beats, ${unforced} without`);
});

test("growth speed is the speed asked for", () => {
  const slow = new PipeField(2);
  const fast = new PipeField(2);
  run(slow, 6, 1);
  run(fast, 6, 6);
  assert.ok(fast.segmentCount > 3 * slow.segmentCount, `${fast.segmentCount} against ${slow.segmentCount}`);
});

test("each piece takes the hue the music had when it was made, shifted by its pipe's own offset", () => {
  const OFFSETS = [0, 0.37, 0.17, 0.55];
  const near = (hue, base) =>
    OFFSETS.some((offset) => Math.abs((((hue - base - offset) % 1) + 1.5) % 1 - 0.5) < 0.01);
  const field = new PipeField(9);
  run(field, 4, 5, 0.1);
  const early = field.segmentCount;
  run(field, 4, 5, 0.7);
  assert.ok(early > 5 && field.segmentCount > early + 5);
  const hueOf = (index) => field.segments[index * PIECE_STRIDE + 7];
  for (let i = 0; i < early; i++) assert.ok(near(hueOf(i), 0.1), `piece ${i}: ${hueOf(i)}`);
  for (let i = early; i < field.segmentCount; i++) assert.ok(near(hueOf(i), 0.7), `piece ${i}: ${hueOf(i)}`);
});

test("when the space fills, the picture fades out and a new one grows", () => {
  const field = new PipeField(4);
  let lowest = 1;
  let restarted = false;
  let peak = 0;
  for (let i = 0; i < 60 * 240 && !restarted; i++) {
    field.step(STEP, 8, 0.4);
    lowest = Math.min(lowest, field.fade);
    peak = Math.max(peak, field.segmentCount);
    if (lowest === 0 && field.segmentCount < 5) restarted = true;
  }
  assert.ok(restarted, "it fades and starts again");
  assert.ok(peak > 150, `${peak} segments before it was full`);
  assert.ok(field.fullness < 0.1);
  run(field, 3);
  assert.equal(field.fade, 1, "and comes back to full strength");
  assert.ok(field.segmentCount > 0);
});

test("the picture is never cleared in the middle of growing while there is room", () => {
  const field = new PipeField(6);
  run(field, 10, 3);
  assert.equal(field.fade, 1);
});

test("the same seed makes the same pipes", () => {
  const a = new PipeField(21);
  const b = new PipeField(21);
  run(a, 12);
  run(b, 12);
  assert.deepEqual(a.segments.slice(0, a.segmentCount * PIECE_STRIDE), b.segments.slice(0, b.segmentCount * PIECE_STRIDE));
});
