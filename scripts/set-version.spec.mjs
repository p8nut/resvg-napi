import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));

test('set-version: every place the version lives, and only the family pins', () => {
  const dir = mkdtempSync(join(tmpdir(), 'set-version-'));
  for (const f of ['package.json', 'npm', 'Cargo.toml', 'Cargo.lock', 'index.js']) cpSync(join(root, f), join(dir, f), { recursive: true });
  const script = join(root, 'scripts/set-version.mjs');
  execFileSync(process.execPath, [script, '1.2.3-rc.1'], { cwd: dir });
  const read = (f) => readFileSync(join(dir, f), 'utf8');
  const pkg = JSON.parse(read('package.json'));
  assert.equal(pkg.version, '1.2.3-rc.1');
  for (const v of Object.values(pkg.optionalDependencies)) assert.equal(v, '1.2.3-rc.1');
  assert.equal(pkg.peerDependencies[`${pkg.name}-wasm32-wasi`], '1.2.3-rc.1');
  assert.ok(Object.values(pkg.dependencies).every((v) => v !== '1.2.3-rc.1'), 'other dependencies untouched');
  assert.equal(JSON.parse(read('npm/linux-x64-gnu/package.json')).version, '1.2.3-rc.1');
  assert.match(read('Cargo.toml'), /^version = "1\.2\.3-rc\.1"/m);
  assert.match(read('Cargo.lock'), /name = "resvg-napi"\nversion = "1\.2\.3-rc\.1"/);
  assert.match(read('index.js'), /!== '1\.2\.3-rc\.1'/);
  assert.doesNotMatch(read('index.js'), /'0\.0\.0'/);
  assert.throws(() => execFileSync(process.execPath, [script, 'v1'], { cwd: dir, stdio: 'pipe' }));
});
