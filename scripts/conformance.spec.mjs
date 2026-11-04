import test from 'node:test';
import assert from 'node:assert/strict';
import { testList, compare, diffPixels, formatBaseline, parseBaseline, THREW } from './conformance.mjs';

const B = (o) => new Map(Object.entries(o));

test('testList: the paths upstream asserts on, sorted, `.svg` appended', () => {
  assert.deepEqual(
    testList('#[test] fn b() { assert_eq!(render("tests/z/b"), 0); }\n' +
             '#[test] fn a() { assert_eq!(render("tests/a"), 0); }\n' +
             'render_extra("tests/skipped")'),
    ['a.svg', 'z/b.svg'],
    'plain render() only, and sorted',
  );
});

test('compare', () => {
  assert.deepEqual(compare(B({ a: 5 }), B({ a: 5 })), { worse: [], better: [], gone: [] });
  assert.deepEqual(compare(B({ a: 5 }), B({ a: 9 })).worse, [{ path: 'a', was: 5, now: 9 }]);
  assert.deepEqual(compare(B({ a: 5 }), B({ a: 0 })).better, [{ path: 'a', was: 5, now: 0 }]);
  // A file absent from the baseline is only news if it differs.
  assert.deepEqual(compare(B({}), B({ n: 0 })).worse, []);
  assert.deepEqual(compare(B({}), B({ n: 3 })).worse, [{ path: 'n', was: 'new', now: 3 }]);
  // A file the corpus no longer has.
  assert.deepEqual(compare(B({ x: 1 }), B({})).gone, ['x']);
});

test('compare: throwing', () => {
  // A case that starts throwing is a regression, whatever it did before.
  assert.deepEqual(compare(B({ a: 0 }), B({ a: THREW })).worse, [{ path: 'a', was: 0, now: 'threw' }]);
  assert.deepEqual(compare(B({ a: 0 }), B({ a: THREW })).better, []);
  // And a new one that throws is not silently accepted.
  assert.deepEqual(compare(B({}), B({ a: THREW })).worse, [{ path: 'a', was: 'new', now: 'threw' }]);
  // One that stops throwing is progress; still throwing is neither.
  assert.deepEqual(compare(B({ a: THREW }), B({ a: 0 })).better, [{ path: 'a', was: 'threw', now: 0 }]);
  const same = compare(B({ a: THREW }), B({ a: THREW }));
  assert.deepEqual([same.worse, same.better], [[], []]);
});

test('the baseline round-trips, and passing files are not written', () => {
  assert.deepEqual([...parseBaseline(formatBaseline(B({ p: 0, f: 7 })))], [['f', 7]]);
});

test('diffPixels: 1 is the same pixel, 2 is not', () => {
  const px = (v) => Uint8Array.from(v);
  assert.equal(diffPixels(px([10, 10, 10, 255]), px([11, 9, 10, 255])), 0);
  assert.equal(diffPixels(px([10, 10, 10, 255]), px([12, 10, 10, 255])), 1);
  assert.equal(diffPixels(px([0, 0, 0, 0]), px([0, 0, 0, 0])), 0);
  assert.equal(diffPixels(px([10, 10, 10, 255]), px([10, 10, 10, 250]), 8), 0, 'alpha counts too');
});
