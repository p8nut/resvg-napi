// Fetches resvg's test corpus at the tag matching the resvg version in
// Cargo.toml. The tag matters: a reference PNG is only a reference for the
// version that produced it.
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const DEST = 'conformance/resvg';

/** The resvg version this crate builds against. */
export function resvgVersion(cargoToml) {
  // Either form, `resvg = "x"` or `resvg = { version = "x", .. }`: Cargo.toml
  // uses the table, once in [dependencies] and once in a target table that adds
  // a feature, and the two must name the same version.
  const found = [...cargoToml.matchAll(/^resvg\s*=\s*(?:\{[^}\n]*?\bversion\s*=\s*)?"([^"]+)"/gm)]
    .map((m) => m[1].replace(/^[\^~=]/, ''));
  if (!found.length) throw new Error('no resvg version in Cargo.toml');
  if (new Set(found).size > 1) throw new Error(`Cargo.toml names resvg at ${found.join(' and ')}`);
  return found[0];
}

// Importable: check-resvg.mjs reuses resvgVersion, and a bare import must not
// re-clone 34MB as a side effect. `import.meta.main` is Node 24+, and the test
// matrix still runs 18.
const main = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];

if (main && process.argv.includes('--selftest')) {
  const assert = (await import('node:assert/strict')).default;
  assert.equal(resvgVersion('resvg = "0.48.1"\n'), '0.48.1');
  assert.equal(resvgVersion('resvg = "^0.48.1"\n'), '0.48.1');
  assert.equal(resvgVersion('a = 1\nresvg = "=0.49.0"  # x\n'), '0.49.0');
  assert.throws(() => resvgVersion('nothing here'));
  const table = 'resvg = { version = "0.48.1", default-features = false }\n[t]\nresvg = { version = "0.48.1", features = ["x"] }\n';
  assert.equal(resvgVersion(table), '0.48.1', 'table form, twice');
  assert.throws(() => resvgVersion(table.replace('"0.48.1", features', '"0.49.0", features')), /0\.48\.1 and 0\.49\.0/);
  console.log('ok — conformance-fetch: 6 checks passed');
} else if (main) {
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
}
