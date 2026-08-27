// Asks crates.io whether resvg has moved past the version Cargo.toml builds
// against, and emits the answer as `key=value` lines so a workflow can read it
// with `>> "$GITHUB_OUTPUT"` and act on it.
//
// Not a red-run notification like check:pins -- there is something to automate
// here, and a failing job would only mean "go do it by hand". The bump job
// opens the PR instead, with the conformance baseline regenerated at the new
// tag: that diff is the whole point, since it is the only thing that says
// whether the rendering changed.
import { readFileSync } from 'node:fs';
import { resvgVersion } from './conformance-fetch.mjs';

const CRATES = 'https://crates.io/api/v1/crates/resvg';

/** `0.48.10` sorts after `0.48.9`, which a string compare gets backwards. */
export function newer(a, b) {
  const pa = a.split('.').map(Number), pb = b.split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d > 0;
  }
  return false;
}

/** The `key=value` lines describing what to do, one per line. */
export function assess(current, latest) {
  const bump = newer(latest, current);
  return [`current=${current}`, `latest=${latest}`, `bump=${bump}`,
          `branch=chore/resvg-${latest}`].join('\n');
}

if (process.argv.includes('--selftest')) {
  const assert = (await import('node:assert/strict')).default;
  assert.ok(newer('0.49.0', '0.48.1'));
  assert.ok(newer('0.48.2', '0.48.1'));
  assert.ok(newer('1.0.0', '0.48.1'));
  // The case a string compare fails.
  assert.ok(newer('0.48.10', '0.48.9'));
  assert.ok(!newer('0.48.9', '0.48.10'));
  assert.ok(!newer('0.48.1', '0.48.1'));
  // Never propose a downgrade: a pin ahead of crates.io stays put.
  assert.ok(!newer('0.48.0', '0.48.1'));
  // Missing components read as zero, so a bare `0.49` is still a bump.
  assert.ok(newer('0.49', '0.48.1'));
  assert.match(assess('0.48.1', '0.49.0'), /^bump=true$/m);
  assert.match(assess('0.48.1', '0.49.0'), /^branch=chore\/resvg-0\.49\.0$/m);
  assert.match(assess('0.48.1', '0.48.1'), /^bump=false$/m);
  console.log('ok — check-resvg: 11 checks passed');
  process.exit(0);
}

const r = await fetch(CRATES, { headers: { 'user-agent': 'resvg-napi check:resvg' } });
if (!r.ok) throw new Error(`${CRATES} -> HTTP ${r.status}`);
const { crate } = await r.json();
console.log(assess(resvgVersion(readFileSync('Cargo.toml', 'utf8')), crate.max_stable_version));
