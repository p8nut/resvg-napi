// Everything the pages need next to them: the wasm, the four bundles, and the
// list the example picker reads. One script rather than a five-command chain in
// package.json, because the Pages workflow builds the same page and static
// hosting cannot answer the `/examples/` route serve.mjs used to.
import { copyFile, readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const here = dirname(fileURLToPath(import.meta.url));  // see render.mjs
const root = join(here, '..');

// Written by `napi build --target wasm32-wasip1-threads`, which the caller runs.
await copyFile(join(root, 'resvg-napi.wasm32-wasi.wasm'),
  join(here, 'resvg-napi.wasm32-wasi.wasm'));

// The loader and its worker are the generated shims; liquid-entry pulls in
// liquidjs, the SVG filters, the QR generator and the helpers (the bench);
// helpers.browser.mjs alone is what `resvg-napi/helpers` resolves to under a
// bundler's `browser` condition (index.html). The loader carries the Buffer
// polyfill, so no page has to install it before importing.
//
// `platform: 'browser'`, which is esbuild's default, said out loud: a `node:`
// import reaching any of these bundles fails the build instead of the page.
for (const [entry, out, inject = []] of [
  ['resvg-napi.wasi-browser.js', 'resvg.js', [join(here, 'buffer-shim.mjs')]],
  ['wasi-worker-browser.mjs', 'wasi-worker-browser.mjs'],
  ['demo/liquid-entry.mjs', 'liquid.js'],
  ['helpers.browser.mjs', 'helpers.js'],
]) {
  await build({
    entryPoints: [join(root, entry)],
    outfile: join(here, out),
    bundle: true,
    format: 'esm',
    platform: 'browser',
    inject,
  });
}

// A fragment (`badge.svg`) has no root <svg> and is not an example: the template
// that renders it pulls it in by name.
const examples = join(here, 'examples');
const files = (await readdir(examples)).filter((f) => f.endsWith('.svg')).sort();
const heads = await Promise.all(files.map((f) =>
  readFile(join(examples, f), 'utf8').then((t) => t.slice(0, 400))));
const names = files.filter((_, i) => /<svg[\s>]/.test(heads[i]));
await writeFile(join(examples, 'index.json'), `${JSON.stringify(names)}\n`);

console.log(`demo/: wasm, 4 bundles, ${names.length} examples`);
