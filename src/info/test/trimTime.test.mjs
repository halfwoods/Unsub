import { test } from "node:test";
import assert from "node:assert/strict";
import { parseTrimTime, trimTime } from "../trimTime.ts";

test("song times parse seconds, minutes, hours, and milliseconds", () => {
  assert.equal(parseTrimTime("12.5"), 12500);
  assert.equal(parseTrimTime("1:02.345"), 62345);
  assert.equal(parseTrimTime("1:02:03.004"), 3723004);
  assert.equal(trimTime(62345), "1:02.345");
  assert.equal(parseTrimTime(trimTime(3723004)), 3723004);
});

test("invalid timestamps are refused", () => {
  for (const value of ["", "-1", "1:60", "1:60:00", "1.5:00", "1:2:3:4", "4.1234", "Infinity"]) {
    assert.equal(parseTrimTime(value), null, value);
  }
});
