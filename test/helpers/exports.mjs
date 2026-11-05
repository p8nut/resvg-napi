// `package.json` `exports` (step 11 of docs/migration-0.10.md), resolved by
// Node itself through the package's own name: what an installed consumer
// gets, under the default conditions and under `browser`, and that a deep
// import is closed.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));

// `import.meta.resolve` is unflagged from Node 20.6; the oldest Node CI runs
// (20.3, `engines`) has it behind a flag only, and resolved nothing without it.
const flag = typeof import.meta.resolve === 'function' ? [] : ['--experimental-import-meta-resolve'];

/** Where `specifier` resolves from inside the package, under `conditions`. */
function resolved(specifier, conditions = []) {
  const child = spawnSync(process.execPath, [
    ...flag,
    ...conditions.map((c) => `--conditions=${c}`),
    '--input-type=module',
    '-e', `try { console.log(import.meta.resolve(${JSON.stringify(specifier)})) } catch (e) { console.log(e.code) }`,
  ], { cwd: root, encoding: 'utf8' });
  assert.equal(child.status, 0, child.stderr);
  return child.stdout.trim().replace(new URL('../../', import.meta.url).href, './');
}

test('exports: Node gets the bound helpers, a bundler the browser variants', () => {
  assert.equal(resolved('resvg-napi'), './index.js');
  assert.equal(resolved('resvg-napi/helpers'), './helpers.mjs');
  assert.equal(resolved('resvg-napi/fit.mjs'), './fit.mjs');
  assert.equal(resolved('resvg-napi/package.json'), './package.json');
  assert.equal(resolved('resvg-napi', ['browser']), './browser.js');
  assert.equal(resolved('resvg-napi/helpers', ['browser']), './helpers.browser.mjs');
  assert.equal(resolved('resvg-napi/fit.mjs', ['browser']), './fit.mjs');
});

test('exports: deep imports are closed', () => {
  for (const deep of ['resvg-napi/index.js', 'resvg-napi/helpers.mjs', 'resvg-napi/fit', 'resvg-napi/resvg-napi.wasi.cjs']) {
    assert.equal(resolved(deep), 'ERR_PACKAGE_PATH_NOT_EXPORTED', deep);
  }
});

test('exports: the package name loads the helpers, bound to the binding', async () => {
  const helpers = await import('resvg-napi/helpers');
  const binding = (await import('resvg-napi')).default;
  assert.equal(helpers.default, helpers.createHelpers(binding));
  const png = helpers.renderPng('<svg xmlns="http://www.w3.org/2000/svg" width="3" height="2"/>', { fonts: [] });
  assert.equal(png.readUInt32BE(16), 3);
});

test('exports: a browser bundle of the helpers loads no binding', async () => {
  const { build } = await import('esbuild');
  const out = await build({
    stdin: { contents: "export { createHelpers, fitTextWidths } from 'resvg-napi/helpers';", resolveDir: root },
    bundle: true, write: false, format: 'esm', platform: 'browser', metafile: true, logLevel: 'silent',
  });
  assert.deepEqual(Object.keys(out.metafile.inputs).sort(), ['<stdin>', 'helpers.browser.mjs']);
});
