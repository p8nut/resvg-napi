import test from 'node:test';
import assert from 'node:assert/strict';
import { assess, unconstrained, anonymous, mismatched, familyPins, wasiDeps } from './check-package.mjs';

test('assess: the root tarball keeps its files[] promises', () => {
  assert.deepEqual(assess({ name: 'a', promised: ['x.js'], shipped: ['x.js', 'package.json'] }), []);
  const missing = assess({ name: 'a', promised: ['x.js'], shipped: ['README.md', 'package.json'] });
  assert.equal(missing.length, 1);
  assert.match(missing[0], /promises x\.js/);
  // A glob is not a promise about one path.
  assert.deepEqual(assess({ name: 'g', promised: ['*.js', 'dist/'], shipped: ['package.json'] }), []);
});

test('assess: no Rust sources in the root tarball', () => {
  const rusty = assess({ name: 'r', promised: [], shipped: ['index.js', 'src/lib.rs', 'build.rs'] });
  assert.equal(rusty.length, 1);
  assert.match(rusty[0], /src\/lib\.rs, build\.rs/);
});

test('unconstrained: an optionalDependency needs os or cpu', () => {
  assert.deepEqual(unconstrained({ a: '1' }, { a: { os: ['linux'], cpu: ['x64'] } }), []);
  // cpu wasm32 with no os is the gate napi CLI 3.10.11 restored: constrained.
  assert.deepEqual(unconstrained({ w: '1' }, { w: { name: 'w', cpu: ['wasm32'] } }), []);
  assert.deepEqual(unconstrained({ a: '1' }, { a: { os: ['linux'] } }), []);
  assert.deepEqual(unconstrained({ a: '1' }, { a: { cpu: ['x64'] } }), []);
  const loose = unconstrained({ w: '1' }, { w: { name: 'w' } });
  assert.equal(loose.length, 1);
  assert.match(loose[0], /installs it everywhere/);
  // A dependency with no platform package of its own is somebody else's problem.
  assert.deepEqual(unconstrained({ pngjs: '1' }, {}), []);
});

test('anonymous: the fields npm renders on a package page', () => {
  const full = { author: 'a', license: 'b', repository: 'c', homepage: 'd', bugs: 'e' };
  assert.deepEqual(anonymous([{ name: 'p', manifest: full }]), []);
  const { author, ...noAuthor } = full;
  const one = anonymous([{ name: 'p', manifest: noAuthor }]);
  assert.equal(one.length, 1);
  assert.match(one[0], /p declares no author/);
  // Every field named at once, not one complaint per field.
  const none = anonymous([{ name: 'p', manifest: {} }]);
  assert.equal(none.length, 1);
  assert.match(none[0], /author, license, repository, homepage, bugs/);
});

test('mismatched: one version across the tree', () => {
  assert.deepEqual(mismatched('1.0.0', { a: '1.0.0' }, { a: { version: '1.0.0' } }), []);
  const stale = mismatched('0.1.1', { a: '0.1.0' }, { a: { version: '0.1.1' } });
  assert.equal(stale.length, 1);
  assert.match(stale[0], /a is pinned at 0\.1\.0, but this package is 0\.1\.1/);
  const behind = mismatched('0.1.1', { a: '0.1.1' }, { a: { version: '0.1.0' } });
  assert.equal(behind.length, 1);
  assert.match(behind[0], /a is 0\.1\.0, but the root is 0\.1\.1/);
  assert.equal(mismatched('0.1.1', { a: '0.1.0', b: '0.1.0' },
    { a: { version: '0.1.1' }, b: { version: '0.1.1' } }).length, 2);
});

test('familyPins: the peer pin 0.3.0 shipped with', () => {
  const peer = mismatched('0.3.0', familyPins({
    name: 'resvg-napi',
    optionalDependencies: { 'resvg-napi-linux-x64-gnu': '0.3.0' },
    peerDependencies: { 'resvg-napi-wasm32-wasi': '0.2.0' },
  }), {});
  assert.equal(peer.length, 1, 'a stale peer pin is a mismatch');
  assert.match(peer[0], /resvg-napi-wasm32-wasi/);
  // Upstream dependencies track upstream: they are not ours to match.
  assert.deepEqual(familyPins({ name: 'resvg-napi', dependencies: { '@emnapi/core': '1.11.3' } }), {});
});

test('wasiDeps: the wasm package and the root agree on the runtime', () => {
  // What the 0.3.1 tag died on: the wasm package left on the previous emnapi.
  assert.equal(wasiDeps({ '@emnapi/core': '1.11.3' }, { '@emnapi/core': '1.11.2' }).length, 1);
  assert.deepEqual(wasiDeps({ '@emnapi/core': '1.11.3' }, { '@emnapi/core': '1.11.3' }), []);
  // A tilde and an exact pin name the same version; only the version counts.
  assert.deepEqual(wasiDeps({ '@napi-rs/wasm-runtime': '1.2.4' }, { '@napi-rs/wasm-runtime': '~1.2.4' }), []);
  // A dependency the root does not share is the wasm package's own business.
  assert.deepEqual(wasiDeps({}, { '@tybys/wasm-util': '0.10.3' }), []);
});
