// The build matrix, and the tier of it a given event deserves.
//
// Two problems, one file.
//
// **The duplication.** `package.json`'s `napi.targets` drives what the CLI
// emits and what `create-npm-dirs` makes; the CI matrix drove what actually got
// built. Adding a platform meant editing both, and nothing checked they agreed
// -- a target in one and not the other fails at publish, or never gets built at
// all. The table below is the single list, and it asserts against
// `napi.targets` rather than trusting itself.
//
// **The cost.** Thirteen targets on every push is most of a CPU-hour to learn
// that a comment changed. A pull request builds the two that can actually tell
// it something -- the one the tests and the conformance suite run on, and wasm,
// whose generated shims and test run are a different code path. Everything else
// waits for the merge, the tag, a manual dispatch, or a `full-matrix` label
// when you want the proof before merging rather than after.
import { readFileSync } from 'node:fs';

/** Host and cross-compilation flag per target. See https://napi.rs/docs/cross-build. */
const TARGETS = {
  'x86_64-unknown-linux-gnu': { host: 'ubuntu-latest' },
  // pure-Rust dependency tree, so napi-cross's glibc 2.17 sysroot is enough
  'aarch64-unknown-linux-gnu': { host: 'ubuntu-latest', args: '--use-napi-cross' },
  'armv7-unknown-linux-gnueabihf': { host: 'ubuntu-latest', args: '--use-napi-cross' },
  // musl goes through zig: napi-cross is glibc only
  'x86_64-unknown-linux-musl': { host: 'ubuntu-latest', args: '-x' },
  'aarch64-unknown-linux-musl': { host: 'ubuntu-latest', args: '-x' },
  'aarch64-apple-darwin': { host: 'macos-latest' },
  'x86_64-apple-darwin': { host: 'macos-latest' },
  // native on a Windows host, so no -x and no cargo-xwin
  'x86_64-pc-windows-msvc': { host: 'windows-latest' },
  'i686-pc-windows-msvc': { host: 'windows-latest' },
  'aarch64-pc-windows-msvc': { host: 'windows-latest' },
  // the CLI configures the NDK from ANDROID_NDK_LATEST_HOME, which the runner
  // image ships -- Android needs no cross flag
  'aarch64-linux-android': { host: 'ubuntu-latest' },
  'armv7-linux-androideabi': { host: 'ubuntu-latest' },
  'wasm32-wasip1-threads': {
    host: 'ubuntu-latest',
    artifact: ['*.wasm', 'resvg-napi.wasi*.*js', 'resvg-napi.wasi.d.cts', 'wasi-worker*.mjs'].join('\n'),
  },
};

/** What a pull request builds: the two that can fail in a way nothing else catches. */
const SLIM = ['x86_64-unknown-linux-gnu', 'wasm32-wasip1-threads'];

const TESTS = [
  { target: 'x86_64-unknown-linux-gnu', host: 'ubuntu-latest', node: 18 },
  { target: 'x86_64-unknown-linux-gnu', host: 'ubuntu-latest', node: 22 },
  { target: 'aarch64-apple-darwin', host: 'macos-latest', node: 22 },
  { target: 'x86_64-pc-windows-msvc', host: 'windows-latest', node: 22 },
];

/** A pull request stays slim unless it asks not to. Everything else is full. */
export function isFull(event, labels) {
  if (event !== 'pull_request') return true;
  return labels.includes('full-matrix');
}

export function matrix(full) {
  const picked = full ? Object.keys(TARGETS) : SLIM;
  return picked.map((target) => ({
    target,
    host: TARGETS[target].host,
    build_args: TARGETS[target].args ?? '',
    artifact_path: TARGETS[target].artifact ?? '*.node',
  }));
}

/** Only targets that were built can be tested. */
export function tests(full) {
  const built = new Set(matrix(full).map((t) => t.target));
  return TESTS.filter((t) => built.has(t.target));
}

/** The table and package.json must name the same platforms. */
export function disagreement(tableTargets, napiTargets) {
  const a = [...tableTargets].sort(), b = [...napiTargets].sort();
  const missing = b.filter((t) => !a.includes(t));
  const extra = a.filter((t) => !b.includes(t));
  const why = [];
  if (missing.length) why.push(`in package.json napi.targets but not in this table: ${missing.join(', ')}`);
  if (extra.length) why.push(`in this table but not in package.json napi.targets: ${extra.join(', ')}`);
  return why;
}

if (process.argv.includes('--selftest')) {
  const assert = (await import('node:assert/strict')).default;
  assert.equal(isFull('push', []), true);
  assert.equal(isFull('schedule', []), true);
  assert.equal(isFull('workflow_dispatch', []), true);
  assert.equal(isFull('pull_request', []), false);
  assert.equal(isFull('pull_request', ['documentation']), false);
  assert.equal(isFull('pull_request', ['full-matrix']), true);
  assert.equal(matrix(false).length, 2);
  assert.equal(matrix(true).length, Object.keys(TARGETS).length);
  // the slim pair is what the drift checks, the test suite and conformance need
  assert.deepEqual(matrix(false).map((t) => t.target).sort(),
    ['wasm32-wasip1-threads', 'x86_64-unknown-linux-gnu']);
  // a target with no flag must still emit the key, or the workflow interpolates
  // the literal string "null" onto the command line
  assert.ok(matrix(true).every((t) => typeof t.build_args === 'string'));
  assert.ok(matrix(true).every((t) => t.artifact_path.length > 0));
  // no test may name a target that was not built
  assert.equal(tests(false).length, 2);
  assert.equal(tests(true).length, 4);
  assert.ok(tests(false).every((t) => t.target === 'x86_64-unknown-linux-gnu'));
  // the consistency check has to fail when it should
  assert.deepEqual(disagreement(['a', 'b'], ['a', 'b']), []);
  assert.match(disagreement(['a'], ['a', 'b'])[0], /not in this table: b/);
  assert.match(disagreement(['a', 'b'], ['a'])[0], /not in package.json.*: b/);
  // and against the real manifest
  const real = JSON.parse(readFileSync('package.json', 'utf8')).napi.targets;
  assert.deepEqual(disagreement(Object.keys(TARGETS), real), [],
    'the table and package.json napi.targets have diverged');
  // GITHUB_OUTPUT takes `key=value` one line at a time; a raw newline in the
  // value silently truncates it and the matrix comes out empty. The wasm
  // artifact path is a multi-line glob, so this is a live hazard, not a
  // hypothetical -- JSON.stringify escaping it is what keeps it on one line.
  for (const v of [JSON.stringify(matrix(true)), JSON.stringify(tests(true))]) {
    assert.ok(!v.includes('\n'), 'a value with a raw newline would truncate GITHUB_OUTPUT');
  }
  assert.ok(matrix(true).find((t) => t.target === 'wasm32-wasip1-threads')
    .artifact_path.includes('\n'), 'and the wasm glob really is multi-line');
  console.log('ok — ci-targets: 21 checks passed');
  process.exit(0);
}

const napiTargets = JSON.parse(readFileSync('package.json', 'utf8')).napi.targets;
const why = disagreement(Object.keys(TARGETS), napiTargets);
if (why.length) {
  for (const w of why) console.error(`::error::${w}`);
  process.exit(1);
}

const full = isFull(process.env.EVENT ?? 'push', JSON.parse(process.env.LABELS || '[]'));
console.log(`full=${full}`);
console.log(`targets=${JSON.stringify(matrix(full))}`);
console.log(`tests=${JSON.stringify(tests(full))}`);
