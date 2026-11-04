// What `npm publish` would ship, checked without publishing.
//
// npm omits a file listed in `files[]` that is not on disk, silently -- so a
// package whose file never arrived publishes without it, and the failure only
// shows up when someone installs it. This checks the root tarball, and the
// manifests of every package. The platform packages' binaries are `napi
// pre-publish --dry-run`'s to check: CI runs it after `napi artifacts`, on the
// runs that build all thirteen.

import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';

/** A `files[]` entry naming one concrete file, not a pattern. */
const isLiteral = (entry) => !/[*?[\]{}]/.test(entry) && !entry.endsWith('/');

/** Rust sources have no business in an npm tarball. */
const RUST = /^(Cargo\.(toml|lock)|build\.rs|src\/)/;

/**
 * The root package: every literal files[] entry in the tarball, and no Rust.
 *
 * @param {{name: string, promised: string[], shipped: string[]}} pkg
 * @returns {string[]} what is wrong, empty when nothing is
 */
export function assess({ name, promised, shipped }) {
  const reasons = [];
  for (const entry of promised.filter(isLiteral)) {
    if (!shipped.includes(entry)) {
      reasons.push(`${name} promises ${entry} in files[] but the tarball has no such path`);
    }
  }
  const rust = shipped.filter((f) => RUST.test(f));
  if (rust.length) reasons.push(`${name} ships Rust sources: ${rust.join(', ')}`);
  return reasons;
}

/**
 * An optionalDependency with no `os` and no `cpu` is not optional in practice:
 * npm has nothing to match against, so it installs it everywhere. The wasm
 * package was in exactly that state from napi CLI 3.8.0 to 3.10.10, so
 * `napi pre-publish` left it out of the list and committing it back put a
 * download on every consumer of every platform. CLI 3.10.11 restored the
 * gate: the wasm package declares `cpu: ["wasm32"]`, npm skips it on every
 * real host, and pre-publish lists it again so that `npm install
 * --cpu=wasm32` can fetch it where no native package exists (StackBlitz,
 * WebContainer). Listed with that `cpu`, it passes here; listed without it,
 * it is the old problem back. The committed root lists it too, so the
 * published manifest is the committed one.
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
 * and, through the `version` script, the platform manifests -- but none of the
 * dependency maps that name them. Left behind, the root asks for platform
 * packages at a version that was never published, and `npm install` resolves
 * nothing. An unpublished version can never be published again, so this is not
 * a mistake a later patch release can undo.
 *
 * `peerDependencies` is checked for the same reason and was the one that got
 * away: it named the wasm package at 0.2.0 through the whole of the 0.3.0
 * release, so `npm i resvg-napi@0.3.0 resvg-napi-wasm32-wasi@0.3.0` answered
 * ERESOLVE for anyone taking the documented WASI path.
 *
 * @param {string} version  the root version
 * @param {Record<string, string>} pinned  every pin naming a package of this family
 * @param {Record<string, {version?: string}>} platforms  by package name
 */
export function mismatched(version, pinned, platforms) {
  const why = [];
  for (const [name, want] of Object.entries(pinned)) {
    if (want !== version) why.push(`${name} is pinned at ${want}, but this package is ${version}`);
  }
  for (const [name, m] of Object.entries(platforms)) {
    if (m.version && m.version !== version) why.push(`${name} is ${m.version}, but the root is ${version}`);
  }
  return why;
}

/**
 * The pins that must move with the version, from wherever they are declared.
 * Restricted to this family by name: `dependencies` also carries `@emnapi/*`
 * and `@napi-rs/wasm-runtime`, which track upstream and have no business
 * matching our version.
 *
 * @param {{name: string, optionalDependencies?: Record<string,string>, peerDependencies?: Record<string,string>, dependencies?: Record<string,string>}} manifest
 */
export function familyPins(manifest) {
  const family = (m) => Object.entries(m ?? {}).filter(([name]) => name.startsWith(manifest.name));
  return Object.fromEntries([
    ...family(manifest.optionalDependencies),
    ...family(manifest.peerDependencies),
    ...family(manifest.dependencies),
  ]);
}

/**
 * The wasm package declares the emnapi trio itself, and `napi pre-publish`
 * refuses to release it when those versions disagree with the root's -- with
 * good reason, since the builder aborts on a mismatched emnapi and the wasm
 * package is the one that carries it to the browser.
 *
 * Nothing writes these but hand: `napi version` moves versions, not
 * dependencies. They were left at 1.11.2 when the root moved to 1.11.3, and
 * the only thing that noticed was the publish job, on the tag, after thirteen
 * targets had already built.
 *
 * @param {Record<string,string>} root  the root's dependencies
 * @param {Record<string,string>} wasm  the wasm package's dependencies
 */
export function wasiDeps(root, wasm) {
  const why = [];
  for (const [name, want] of Object.entries(wasm ?? {})) {
    const mine = (root ?? {})[name];
    if (!mine) continue;
    // `~1.2.4` and `1.2.4` are both acceptable spellings of the same intent;
    // what matters is the version they name.
    if (want.replace(/^[~^]/, '') !== mine.replace(/^[~^]/, '')) {
      why.push(`the wasm package declares ${name} ${want}, but the root uses ${mine}`);
    }
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

if (import.meta.main) {
  const root = JSON.parse(readFileSync('package.json', 'utf8'));
  const shipped = pack('.');
  const platforms = Object.fromEntries(readdirSync('npm')
    .filter((d) => existsSync(join('npm', d, 'package.json')))
    .map((d) => {
      const m = JSON.parse(readFileSync(join('npm', d, 'package.json'), 'utf8'));
      return [m.name, m];
    }));
  const wasi = platforms[`${root.name}-wasm32-wasi`];
  const reasons = [
    ...assess({ name: root.name, promised: root.files ?? [], shipped }),
    ...unconstrained(root.optionalDependencies ?? {}, platforms),
    ...anonymous([
      { name: root.name, manifest: root },
      ...Object.entries(platforms).map(([name, manifest]) => ({ name, manifest })),
    ]),
    ...mismatched(root.version, familyPins(root), platforms),
    ...(wasi ? wasiDeps(root.dependencies, wasi.dependencies) : []),
  ];
  console.log(`  ${root.name}: ${shipped.length} file(s); ${Object.keys(platforms).length} platform manifests`);
  if (reasons.length) {
    console.error('\nWhat would be published is wrong:\n');
    for (const r of reasons) console.error(`  - ${r}`);
    process.exit(1);
  }
  console.log('every files[] promise kept, every manifest in step');
}
