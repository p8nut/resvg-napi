import test from 'node:test';
import assert from 'node:assert/strict';
import { entry, isFull, matrix, tests, emit } from './ci-targets.mjs';

test('only an unlabelled pull request is slim', () => {
  assert.equal(isFull('push', []), true);
  assert.equal(isFull('schedule', []), true);
  assert.equal(isFull('workflow_dispatch', []), true);
  assert.equal(isFull('pull_request', []), false);
  assert.equal(isFull('pull_request', ['documentation']), false);
  assert.equal(isFull('pull_request', ['full-matrix']), true);
});

test('host and flag follow from the triple', () => {
  const pick = (t) => { const { host, build_args } = entry(t); return [host, build_args]; };
  assert.deepEqual(pick('x86_64-unknown-linux-gnu'), ['ubuntu-latest', '--use-napi-cross']);
  assert.deepEqual(pick('armv7-unknown-linux-gnueabihf'), ['ubuntu-latest', '--use-napi-cross']);
  assert.deepEqual(pick('aarch64-unknown-linux-musl'), ['ubuntu-latest', '-x']);
  assert.deepEqual(pick('aarch64-apple-darwin'), ['macos-latest', '']);
  assert.deepEqual(pick('i686-pc-windows-msvc'), ['windows-latest', '']);
  assert.deepEqual(pick('armv7-linux-androideabi'), ['ubuntu-latest', '']);
  assert.equal(entry('x86_64-apple-darwin').artifact_path, '*.node');
  assert.match(entry('wasm32-wasip1-threads').artifact_path, /^\*\.wasm\n/);
});

test('the matrix is napi.targets, or the slim pair', () => {
  assert.deepEqual(matrix(true, ['a-apple-darwin']).map((t) => t.target), ['a-apple-darwin']);
  assert.deepEqual(matrix(false).map((t) => t.target).sort(),
    ['wasm32-wasip1-threads', 'x86_64-unknown-linux-gnu']);
  // A target with no flag still emits the key, or the workflow interpolates
  // the literal string "null" onto the command line.
  assert.ok(matrix(true).every((t) => typeof t.build_args === 'string'));
});

test('no test names a target that was not built', () => {
  assert.equal(tests(false).length, 3);
  assert.equal(tests(true).length, 5);
  assert.ok(tests(false).every((t) => t.target === 'x86_64-unknown-linux-gnu'));
});

// GITHUB_OUTPUT takes `key=value` one line at a time; a raw newline in a value
// silently truncates it and the matrix comes out empty. The wasm artifact path
// is a multi-line glob, so this is a live hazard.
test('every output is one key=value line', () => {
  const lines = emit(true).trimEnd().split('\n');
  assert.equal(lines.length, 3);
  for (const line of lines) assert.match(line, /^[a-z]+=/);
});
