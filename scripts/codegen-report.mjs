// The codegen's report, snapshotted.
//
// `build.rs` says what it derived from upstream and what it left alone, with
// the reason for each -- and the README now points at that report as the answer
// to "why is this member missing". So it is documentation, and nothing was
// pinning it: a usvg bump could add or drop a line unnoticed. CI already fails
// on a diff in the generated *output*; this does the same for the intent.
//
//   npm run report    rewrite codegen-report.txt; the diff shows what moved
//
// CI runs it and fails on a diff, the check the generated bindings get too.

import { readFileSync, writeFileSync, utimesSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

/** The Cargo package name, as cargo prints it in `warning: <name>@<version>:`. */
const crateName = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8')
  .match(/^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms)[1];

const warning = `warning: ${crateName}@`;

const SNAPSHOT = 'codegen-report.txt';

/**
 * The report lines, machine-independent.
 *
 * One line names where usvg was read from, as an absolute path into the cargo
 * registry -- different on every machine, and the crate directory is the only
 * part worth pinning: it carries the version, so a bump shows up as a diff.
 */
export function normalise(stderr) {
  return stderr
    .split('\n')
    // Belt as well as braces: the spawn forces colour off, but a line that
    // arrives decorated anyway must not be dropped on the floor.
    .map((l) => l.replace(/\u001b\[[0-9;]*m/g, ''))
    .filter((l) => l.startsWith(warning))
    .map((l) => l.slice(l.indexOf(' ', warning.length) + 1))
    .map((l) => l.replace(/(usvg sources: ).*[/\\]([^/\\]+)[/\\]src$/, '$1$2'))
    .sort()
    .join('\n')
    .concat('\n');
}

function generate() {
  // The report only prints when build.rs reruns, and it only reruns when it
  // looks newer than its output.
  const now = new Date();
  utimesSync('build.rs', now, now);
  // spawnSync, not execFileSync: the report rides on stderr, and execFileSync
  // only hands stderr back when the command *fails*.
  const r = spawnSync('cargo', ['build', '--release'], {
    // CARGO_TERM_COLOR=always is set in CI, and cargo then prefixes every line
    // with escape codes -- which is why the shell version of this step matched
    // nothing there and printed nothing, silently, for as long as it existed.
    env: { ...process.env, RESVG_NAPI_CODEGEN_LOG: '1', CARGO_TERM_COLOR: 'never' },
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  });
  if (r.error) throw r.error;
  if (r.status !== 0) {
    process.stderr.write(r.stderr ?? '');
    console.error('cargo build failed; nothing to snapshot');
    process.exit(r.status ?? 1);
  }
  return r.stderr ?? '';
}

if (import.meta.main) {
  const got = normalise(generate());
  if (!got.trim()) {
    console.error('the codegen printed no report — did build.rs rerun?');
    process.exit(1);
  }
  writeFileSync(SNAPSHOT, got);
  console.log(`${SNAPSHOT} written, ${got.trim().split('\n').length} lines`);
}
