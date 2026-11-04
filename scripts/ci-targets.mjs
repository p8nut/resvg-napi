// The build matrix, and the tier of it a given event deserves.
//
// The targets are package.json's `napi.targets`, the list the CLI emits loaders
// for and `create-npm-dirs` makes packages for. The host and the cross flag
// follow from the triple (see https://napi.rs/docs/cross-build), so there is no
// second list to keep in step with it.
//
// Thirteen targets on every push is most of a CPU-hour to learn that a comment
// changed. A pull request builds the two that can actually tell it something --
// the one the tests and the conformance suite run on, and wasm, whose generated
// shims and test run are a different code path. Everything else waits for the
// merge, the tag, a manual dispatch, or a `full-matrix` label when you want the
// proof before merging rather than after.
import { readFileSync } from 'node:fs';

const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
/** The file stem napi gives the binaries and the WASI shims. */
const binaryName = pkg.napi?.binaryName ?? pkg.name;

/** Host, cross flag and upload glob for one triple. */
export function entry(target) {
  const host = target.includes('-darwin') ? 'macos-latest'
    : target.includes('-windows') ? 'windows-latest'
    : 'ubuntu-latest';
  // glibc goes through napi-cross for its 2.17 sysroot, x64 included: built
  // natively it linked the runner's glibc, and 0.5.0 needed 2.35. musl goes
  // through zig, since napi-cross is glibc only. Windows builds natively on a
  // Windows host, and Android needs no flag: the CLI finds the runner's NDK.
  const build_args = target.includes('-linux-gnu') ? '--use-napi-cross'
    : target.includes('-musl') ? '-x'
    : '';
  const artifact_path = target.startsWith('wasm32-')
    ? ['*.wasm', `${binaryName}.wasi*.*js`, `${binaryName}.wasi.d.cts`, 'wasi-worker*.mjs'].join('\n')
    : '*.node';
  return { target, host, build_args, artifact_path };
}

/** What a pull request builds: the two that can fail in a way nothing else catches. */
const SLIM = ['x86_64-unknown-linux-gnu', 'wasm32-wasip1-threads'];

const TESTS = [
  // The engines floor exactly, not the latest 20.x: napi9 needs 20.3.
  { target: 'x86_64-unknown-linux-gnu', host: 'ubuntu-latest', node: '20.3.0' },
  { target: 'x86_64-unknown-linux-gnu', host: 'ubuntu-latest', node: 22 },
  { target: 'x86_64-unknown-linux-gnu', host: 'ubuntu-latest', node: 24 },
  { target: 'aarch64-apple-darwin', host: 'macos-latest', node: 22 },
  { target: 'x86_64-pc-windows-msvc', host: 'windows-latest', node: 22 },
];

/** A pull request stays slim unless it asks not to. Everything else is full. */
export function isFull(event, labels) {
  if (event !== 'pull_request') return true;
  return labels.includes('full-matrix');
}

export function matrix(full, targets = pkg.napi.targets) {
  return (full ? targets : SLIM).map(entry);
}

/** Only targets that were built can be tested. */
export function tests(full, targets = pkg.napi.targets) {
  const built = new Set(matrix(full, targets).map((t) => t.target));
  return TESTS.filter((t) => built.has(t.target));
}

/** What the script writes to GITHUB_OUTPUT: one `key=value` per line. */
export function emit(full, targets = pkg.napi.targets) {
  return [
    `full=${full}`,
    `targets=${JSON.stringify(matrix(full, targets))}`,
    `tests=${JSON.stringify(tests(full, targets))}`,
  ].join('\n') + '\n';
}

if (import.meta.main) {
  const full = isFull(process.env.EVENT ?? 'push', JSON.parse(process.env.LABELS || '[]'));
  process.stdout.write(emit(full));
}
