// Fetches resvg's test corpus at the tag matching the resvg version Cargo.lock
// builds. The tag matters: a reference PNG is only a reference for the version
// that produced it.
import { existsSync, rmSync } from 'node:fs';
import { execFileSync, spawnSync } from 'node:child_process';

const DEST = 'conformance/resvg';

// `registry+https://...crates.io-index#resvg@0.49.0`
const tag = `v${execFileSync('cargo', ['pkgid', 'resvg'], { encoding: 'utf8' }).trim().split('@').pop()}`;

const git = (...args) => spawnSync('git', args, { stdio: ['ignore', 'pipe', 'inherit'], encoding: 'utf8' });

const have = existsSync(DEST) ? git('-C', DEST, 'describe', '--tags', '--exact-match', 'HEAD').stdout?.trim() : null;
if (have === tag) {
  console.log(`corpus at ${tag} already in ${DEST}`);
} else {
  if (existsSync(DEST)) rmSync(DEST, { recursive: true, force: true });
  const run = (...args) => {
    const r = spawnSync('git', args, { stdio: 'inherit' });
    if (r.status !== 0) process.exit(r.status ?? 1);
  };
  // Blobless and sparse: the corpus is 34MB of the repo, the history is not
  // wanted at all.
  run('clone', '--depth', '1', '--branch', tag, '--filter=blob:none', '--sparse',
      'https://github.com/linebender/resvg.git', DEST);
  run('-C', DEST, 'sparse-checkout', 'set', 'crates/resvg/tests');
  console.log(`corpus at ${tag} in ${DEST}`);
}
