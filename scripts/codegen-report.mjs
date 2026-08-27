// The codegen's report, snapshotted.
//
// `build.rs` says what it derived from upstream and what it left alone, with
// the reason for each -- and the README now points at that report as the answer
// to "why is this member missing". So it is documentation, and nothing was
// pinning it: a usvg bump could add or drop a line unnoticed. CI already fails
// on a diff in the generated *output*; this does the same for the intent.
//
//   npm run report          compare against codegen-report.txt
//   npm run report -- -w    rewrite it, after a change you meant
//   npm run report -- -s    selftest, offline
//
// Regenerating needs a cargo build; only the comparison is interesting to CI.

import { readFileSync, writeFileSync, utimesSync, existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

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
    .filter((l) => l.startsWith('warning: resvg-napi@'))
    .map((l) => l.replace(/^warning: resvg-napi@[^ ]* /, ''))
    .map((l) => l.replace(/(usvg sources: ).*[/\\]([^/\\]+)[/\\]src$/, '$1$2'))
    .sort()
    .join('\n')
    .concat('\n');
}

/** What to say about a snapshot that no longer matches. */
export function compare(want, got) {
  if (want === got) return null;
  const w = new Set(want.trim().split('\n'));
  const g = new Set(got.trim().split('\n'));
  const gone = [...w].filter((l) => !g.has(l));
  const fresh = [...g].filter((l) => !w.has(l));
  return { gone, fresh };
}

function generate() {
  // The report only prints when build.rs reruns, and it only reruns when it
  // looks newer than its output.
  const now = new Date();
  utimesSync('build.rs', now, now);
  // spawnSync, not execFileSync: the report rides on stderr, and execFileSync
  // only hands stderr back when the command *fails*.
  const r = spawnSync('cargo', ['build', '--release'], {
    env: { ...process.env, RESVG_NAPI_CODEGEN_LOG: '1' },
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

async function selftest() {
  const assert = (await import('node:assert/strict')).default;
  const raw = [
    'warning: resvg-napi@0.1.0: usvg sources: /home/x/.cargo/registry/src/idx-abc/usvg-0.48.1/src',
    'warning: resvg-napi@0.1.0: b line',
    'warning: some other crate: not ours',
    'plain cargo noise',
  ].join('\n');

  const out = normalise(raw);
  assert.equal(out, 'b line\nusvg sources: usvg-0.48.1\n',
    'ours only, sorted, and the registry path reduced to the crate dir');

  // the version survives, so a bump is a diff rather than a silence
  assert.match(normalise(raw), /usvg-0\.48\.1/);
  const bumped = normalise(raw.replace('0.48.1', '0.49.0'));
  assert.ok(compare(out, bumped), 'a version bump does not compare equal');

  assert.equal(compare('a\n', 'a\n'), null, 'identical is silent');
  assert.deepEqual(compare('a\nb\n', 'a\n'), { gone: ['b'], fresh: [] });
  assert.deepEqual(compare('a\n', 'a\nc\n'), { gone: [], fresh: ['c'] });

  // a Windows path reduces the same way
  assert.match(
    normalise('warning: resvg-napi@0.1.0: usvg sources: C:\\c\\registry\\src\\i\\usvg-1.0.0\\src'),
    /usvg sources: usvg-1\.0\.0/,
  );

  console.log('ok — codegen-report: 7 checks passed');
}

async function main() {
  const write = process.argv.includes('-w') || process.argv.includes('--write');
  const got = normalise(generate());
  if (!got.trim()) {
    console.error('the codegen printed no report — did build.rs rerun?');
    process.exit(1);
  }
  if (write || !existsSync(SNAPSHOT)) {
    writeFileSync(SNAPSHOT, got);
    console.log(`${SNAPSHOT} written, ${got.trim().split('\n').length} lines`);
    return;
  }
  const diff = compare(readFileSync(SNAPSHOT, 'utf8'), got);
  if (!diff) {
    console.log(`${SNAPSHOT} matches, ${got.trim().split('\n').length} lines`);
    return;
  }
  console.error(`${SNAPSHOT} is out of date. The derived surface changed:\n`);
  for (const l of diff.gone) console.error(`  - ${l}`);
  for (const l of diff.fresh) console.error(`  + ${l}`);
  console.error(`\nIf that was the point, rerun with: npm run report -- -w`);
  process.exit(1);
}

const arg = process.argv.includes('-s') || process.argv.includes('--selftest');
await (arg ? selftest() : main());
