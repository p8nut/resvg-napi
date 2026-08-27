// Fetches resvg's test corpus at the tag matching the resvg version in
// Cargo.toml. The tag matters: a reference PNG is only a reference for the
// version that produced it.
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

const DEST = 'conformance/resvg';

/** The resvg version this crate builds against. */
export function resvgVersion(cargoToml) {
  const m = cargoToml.match(/^resvg\s*=\s*"([^"]+)"/m);
  if (!m) throw new Error('no resvg version in Cargo.toml');
  return m[1].replace(/^[\^~=]/, '');
}

if (process.argv.includes('--selftest')) {
  const assert = (await import('node:assert/strict')).default;
  assert.equal(resvgVersion('resvg = "0.48.1"\n'), '0.48.1');
  assert.equal(resvgVersion('resvg = "^0.48.1"\n'), '0.48.1');
  assert.equal(resvgVersion('a = 1\nresvg = "=0.49.0"  # x\n'), '0.49.0');
  assert.throws(() => resvgVersion('nothing here'));
  console.log('ok — conformance-fetch: 4 checks passed');
  process.exit(0);
}

const tag = `v${resvgVersion(readFileSync('Cargo.toml', 'utf8'))}`;
if (existsSync(DEST)) rmSync(DEST, { recursive: true, force: true });

// Blobless and sparse: the corpus is 34MB of the repo, the history is not
// wanted at all.
const git = (...args) => {
  const r = spawnSync('git', args, { stdio: 'inherit' });
  if (r.status !== 0) process.exit(r.status ?? 1);
};
git('clone', '--depth', '1', '--branch', tag, '--filter=blob:none', '--sparse',
    'https://github.com/linebender/resvg.git', DEST);
git('-C', DEST, 'sparse-checkout', 'set', 'crates/resvg/tests');
console.log(`corpus at ${tag} in ${DEST}`);
