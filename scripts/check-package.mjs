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

/**
 * The fields npm renders on a package page. Absent, they do not fail anything --
 * the package publishes and simply shows no author and no way to report a bug.
 * This repository shipped all the way to a release candidate with every one of
 * them missing, which is how quiet the failure is.
 *
 * @param {{name: string, manifest: Record<string, unknown>}[]} pkgs
 */
export function anonymous(pkgs) {
  const wanted = ['author', 'license', 'repository', 'homepage', 'bugs'];
  return pkgs.flatMap(({ name, manifest }) => {
    const missing = wanted.filter((k) => !manifest[k]);
    return missing.length ? [`${name} declares no ${missing.join(', ')}`] : [];
  });
}

/**
 * Every version in the tree must be the same one. `npm version` bumps the root
 * and, through the `version` script, the platform manifests -- but not the
 * optionalDependencies that name them. Left behind, the root asks for platform
 * packages at a version that was never published, and `npm install` resolves
 * nothing. An unpublished version can never be published again, so this is not
 * a mistake a later patch release can undo.
 *
 * @param {string} version  the root version
 * @param {Record<string, string>} optional  optionalDependencies
 * @param {Record<string, {version?: string}>} platforms  by package name
 */
export function mismatched(version, optional, platforms) {
  const why = [];
  for (const [name, want] of Object.entries(optional)) {
    if (want !== version) why.push(`${name} is pinned at ${want}, but this package is ${version}`);
  }
  for (const [name, m] of Object.entries(platforms)) {
    if (m.version && m.version !== version) why.push(`${name} is ${m.version}, but the root is ${version}`);
  }
  return why;
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
  const full = { author: 'a', license: 'b', repository: 'c', homepage: 'd', bugs: 'e' };
  assert.deepEqual(anonymous([{ name: 'p', manifest: full }]), []);
  const { author, ...noAuthor } = full;
  const one = anonymous([{ name: 'p', manifest: noAuthor }]);
  assert.equal(one.length, 1);
  assert.match(one[0], /p declares no author/);
  // every field named at once, not one complaint per field
  const none = anonymous([{ name: 'p', manifest: {} }]);
  assert.equal(none.length, 1);
  assert.match(none[0], /author, license, repository, homepage, bugs/);
  assert.deepEqual(mismatched('1.0.0', { a: '1.0.0' }, { a: { version: '1.0.0' } }), []);
  const stale = mismatched('0.1.1', { a: '0.1.0' }, { a: { version: '0.1.1' } });
  assert.equal(stale.length, 1);
  assert.match(stale[0], /a is pinned at 0\.1\.0, but this package is 0\.1\.1/);
  const behind = mismatched('0.1.1', { a: '0.1.1' }, { a: { version: '0.1.0' } });
  assert.equal(behind.length, 1);
  assert.match(behind[0], /a is 0\.1\.0, but the root is 0\.1\.1/);
  // exactly what `npm version patch` leaves behind: manifests bumped, pins not
  assert.equal(mismatched('0.1.1', { a: '0.1.0', b: '0.1.0' },
    { a: { version: '0.1.1' }, b: { version: '0.1.1' } }).length, 2);
  console.log('ok — check-package: 23 checks passed');
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
  const drift = mismatched(root.version, root.optionalDependencies ?? {}, platforms);
  const bare = anonymous([
    { name: root.name, manifest: root },
    ...(rootOnly ? [] : Object.entries(platforms).map(([name, manifest]) => ({ name, manifest }))),
  ]);
  if (rootOnly) console.log('  (root package only -- not every platform binary is present)');
  const reasons = [...assess(pkgs), ...loose, ...bare, ...drift];
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
