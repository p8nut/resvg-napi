// What `npm publish` would ship, checked without publishing.
//
// npm omits a file listed in `files[]` that is not on disk, silently -- so a
// platform package whose binary never arrived publishes as a README and a
// manifest, and the failure only shows up when someone installs it. The publish
// job runs `napi artifacts` to place those binaries; nothing verified that it
// worked, because that job only ever runs on a `v*` tag.
//
// `--selftest` exercises the decision offline; the real run needs `npm pack`.

import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';

/** A `files[]` entry naming one concrete file, not a pattern. */
const isLiteral = (entry) => !/[*?[\]{}]/.test(entry) && !entry.endsWith('/');

/** Rust sources have no business in an npm tarball. */
const RUST = /^(Cargo\.(toml|lock)|build\.rs|src\/)/;

/**
 * @param {{name: string, promised: string[], shipped: string[], root: boolean}[]} pkgs
 * @returns {string[]} what is wrong, empty when nothing is
 */
export function assess(pkgs) {
  const reasons = [];
  for (const { name, promised, shipped, root } of pkgs) {
    for (const entry of promised.filter(isLiteral)) {
      if (!shipped.includes(entry)) {
        reasons.push(`${name} promises ${entry} in files[] but the tarball has no such path`);
      }
    }
    if (shipped.length === 0) {
      reasons.push(`${name} would publish an empty tarball`);
    }
    if (root) {
      const rust = shipped.filter((f) => RUST.test(f));
      if (rust.length) {
        reasons.push(`${name} ships Rust sources: ${rust.join(', ')}`);
      }
    }
  }
  return reasons;
}

/**
 * An optionalDependency with no `os` and no `cpu` is not optional in practice:
 * npm has nothing to match against, so it installs it everywhere. That is why
 * `napi pre-publish` drops the wasm package from the list -- it is reached
 * through `browser.js` and the WASI shim, not through per-platform resolution.
 * Committing it back put a download on every consumer of every platform, and
 * made the published manifest differ from the committed one on every release.
 *
 * @param {Record<string, string>} optional  optionalDependencies
 * @param {Record<string, {os?: string[], cpu?: string[]}>} platforms  by package name
 */
export function unconstrained(optional, platforms) {
  return Object.keys(optional)
    .filter((name) => {
      const m = platforms[name];
      return m && !(m.os?.length || m.cpu?.length);
    })
    .map((name) => `${name} is an optionalDependency with no os/cpu, so npm installs it everywhere`);
}

function pack(dir) {
  const out = execFileSync('npm', ['pack', '--dry-run', '--json'], {
    cwd: dir,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  });
  return JSON.parse(out)[0].files.map((f) => f.path);
}

// `--root-only` checks the root package and nothing else. A slim CI run holds
// two of the thirteen platform binaries, so the per-platform half would report
// eleven absences that are not faults -- while the half that matters, the
// files[] entries npm drops in silence, still runs.
function collect(rootOnly = false) {
  const read = (d) => JSON.parse(readFileSync(join(d, 'package.json'), 'utf8'));
  const pkgs = [{ name: read('.').name, promised: read('.').files ?? [], shipped: pack('.'), root: true }];
  if (rootOnly) return pkgs;
  for (const d of readdirSync('npm')) {
    const dir = join('npm', d);
    if (!existsSync(join(dir, 'package.json'))) continue;
    const m = read(dir);
    pkgs.push({ name: m.name, promised: m.files ?? [], shipped: pack(dir), root: false });
  }
  return pkgs;
}

async function selftest() {
  const assert = (await import('node:assert/strict')).default;

  assert.deepEqual(
    assess([{ name: 'a', promised: ['x.node'], shipped: ['x.node', 'package.json'], root: false }]),
    [],
    'a promise the tarball keeps is fine',
  );

  const missing = assess([
    { name: 'a', promised: ['x.node'], shipped: ['README.md', 'package.json'], root: false },
  ]);
  assert.equal(missing.length, 1);
  assert.match(missing[0], /promises x\.node/);

  const empty = assess([{ name: 'a', promised: [], shipped: [], root: false }]);
  assert.equal(empty.length, 1);
  assert.match(empty[0], /empty tarball/);

  const rusty = assess([
    { name: 'r', promised: [], shipped: ['index.js', 'src/lib.rs', 'build.rs'], root: true },
  ]);
  assert.equal(rusty.length, 1);
  assert.match(rusty[0], /src\/lib\.rs, build\.rs/);

  // Rust in a platform package is not checked -- only the root claims to be
  // source-free, and this is the rule that says so.
  assert.deepEqual(
    assess([{ name: 'p', promised: [], shipped: ['build.rs'], root: false }]),
    [],
  );

  // a glob is not a promise about one path
  assert.deepEqual(
    assess([{ name: 'g', promised: ['*.node', 'dist/'], shipped: ['package.json'], root: false }]),
    [],
  );

  // the invariant that would have caught the wasm package being put back
  assert.deepEqual(unconstrained({ a: '1' }, { a: { os: ['linux'], cpu: ['x64'] } }), []);
  assert.deepEqual(unconstrained({ a: '1' }, { a: { os: ['linux'] } }), []);
  assert.deepEqual(unconstrained({ a: '1' }, { a: { cpu: ['x64'] } }), []);
  const loose = unconstrained({ w: '1' }, { w: { name: 'w' } });
  assert.equal(loose.length, 1);
  assert.match(loose[0], /installs it everywhere/);
  // a dependency with no platform package of its own is somebody else's problem
  assert.deepEqual(unconstrained({ pngjs: '1' }, {}), []);
  console.log('ok — check-package: 12 checks passed');
}

async function main() {
  const rootOnly = process.argv.includes('--root-only');
  const pkgs = collect(rootOnly);
  const platforms = Object.fromEntries(readdirSync('npm')
    .filter((d) => existsSync(join('npm', d, 'package.json')))
    .map((d) => {
      const m = JSON.parse(readFileSync(join('npm', d, 'package.json'), 'utf8'));
      return [m.name, m];
    }));
  const root = JSON.parse(readFileSync('package.json', 'utf8'));
  const loose = unconstrained(root.optionalDependencies ?? {}, platforms);
  if (rootOnly) console.log('  (root package only -- not every platform binary is present)');
  const reasons = [...assess(pkgs), ...loose];
  for (const p of pkgs) {
    console.log(`  ${p.name.padEnd(30)} ${p.shipped.length} file(s)`);
  }
  if (reasons.length) {
    console.error('\nWhat would be published is wrong:\n');
    for (const r of reasons) console.error(`  - ${r}`);
    process.exit(1);
  }
  console.log(`\n${pkgs.length} packages, every files[] promise kept`);
}

await (process.argv.includes('--selftest') ? selftest() : main());
