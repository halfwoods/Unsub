import assert from "node:assert/strict";
import { test } from "node:test";

import { EDGE_STRIDE, MAX_EDGES, POLYGON_SIDES, Polygons, TRAIL_COPIES } from "../polygons.ts";

const STEP = 1 / 60;

/** A deterministic stand-in for Math.random. */
function sequence(seed = 1) {
  let state = seed;
  return () => {
    state = (state * 1664525 + 1013904223) % 4294967296;
    return state / 4294967296;
  };
}

function run(polygons, seconds, aspect, speed) {
  for (let i = 0; i < Math.round(seconds / STEP); i++) polygons.step(STEP, aspect, speed);
}

function edges(polygons) {
  const out = new Float32Array(MAX_EDGES * EDGE_STRIDE);
  return { out, count: polygons.fill(out) };
}

test("corners bounce off the edges of the picture and never leave it", () => {
  const polygons = new Polygons(sequence(3));
  for (const aspect of [1.78, 1.78, 0.8, 2.4]) {
    run(polygons, 20, aspect, 1.1);
    const { out, count } = edges(polygons);
    for (let i = 0; i < count; i++) {
      // The live polygon is the one that has had its say about the bounds just now.
      if (out[i * EDGE_STRIDE + 4] !== 0) continue;
      for (const [x, y] of [[0, 1], [2, 3]]) {
        assert.ok(Math.abs(out[i * EDGE_STRIDE + x]) <= aspect + 1e-4, `x ${out[i * EDGE_STRIDE + x]} at aspect ${aspect}`);
        assert.ok(Math.abs(out[i * EDGE_STRIDE + y]) <= 1 + 1e-4);
      }
    }
  }
});

test("corners cover ground at the speed asked for", () => {
  const moved = (speed) => {
    const polygons = new Polygons(sequence(5));
    const before = edges(polygons);
    polygons.step(0.05, 100, speed);
    const after = edges(polygons);
    let total = 0;
    let corners = 0;
    for (let i = 0; i < POLYGON_SIDES[0]; i++) {
      // The live polygon's edges come first, each starting at a corner.
      total += Math.hypot(after.out[i * EDGE_STRIDE] - before.out[i * EDGE_STRIDE], after.out[i * EDGE_STRIDE + 1] - before.out[i * EDGE_STRIDE + 1]);
      corners++;
    }
    return total / corners / 0.05;
  };
  assert.ok(Math.abs(moved(0.2) - 0.2) < 0.01, `${moved(0.2)}`);
  assert.ok(Math.abs(moved(0.9) - 0.9) < 0.01, `${moved(0.9)}`);
});

test("a trail builds up behind each polygon and stops at its length", () => {
  const polygons = new Polygons(sequence(7));
  assert.equal(edges(polygons).count, POLYGON_SIDES.reduce((a, b) => a + b, 0), "only the polygons to begin with");
  run(polygons, 0.5, 1.78, 0.5);
  const grown = edges(polygons).count;
  assert.ok(grown > 10 * POLYGON_SIDES[0]);
  run(polygons, 5, 1.78, 0.5);
  const { out, count } = edges(polygons);
  assert.equal(count, MAX_EDGES);
  assert.equal(count, POLYGON_SIDES.reduce((total, sides) => total + sides * (TRAIL_COPIES + 1), 0));
  const ages = new Set();
  for (let i = 0; i < count; i++) ages.add(out[i * EDGE_STRIDE + 4].toFixed(4));
  assert.equal(ages.size, TRAIL_COPIES + 1);
  assert.ok(Math.max(...[...ages].map(Number)) <= 1 && Math.min(...[...ages].map(Number)) === 0);
});

test("a stall does not fill the trail with copies of one place", () => {
  const polygons = new Polygons(sequence(9));
  polygons.step(5, 1.78, 0.5);
  assert.ok(edges(polygons).count <= POLYGON_SIDES.reduce((total, sides) => total + sides * 4, 0));
});

test("a kick swerves the corners and speeds them up, and the speed dies away", () => {
  const travelled = (kick) => {
    const polygons = new Polygons(sequence(11));
    if (kick) polygons.kick(1);
    const before = edges(polygons).out.slice(0, 2);
    polygons.step(0.05, 100, 0.3);
    const after = edges(polygons).out.slice(0, 2);
    return Math.hypot(after[0] - before[0], after[1] - before[1]) / 0.05;
  };
  assert.ok(travelled(true) > 2.5 * travelled(false), `${travelled(true)} against ${travelled(false)}`);

  const polygons = new Polygons(sequence(11));
  polygons.kick(1);
  run(polygons, 2, 100, 0.3);
  const before = edges(polygons).out.slice(0, 2);
  polygons.step(0.05, 100, 0.3);
  const after = edges(polygons).out.slice(0, 2);
  assert.ok(Math.abs(Math.hypot(after[0] - before[0], after[1] - before[1]) / 0.05 - 0.3) < 0.02);
});
