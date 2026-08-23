// Image resolution: two-pass hook (Send+Sync forbids a JS callback inside usvg).
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
const { Resvg } = createRequire(import.meta.url)('./index.js');

// a 20x20 red PNG, produced by our own renderer
const red = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="#ff0000"/></svg>').renderPng();

const doc = (href) => `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">
  <image xlink:href="${href}" x="0" y="0" width="20" height="20"/>
</svg>`;

const pixel = (r) => [...r.renderRaw().data.subarray(0, 4)];

// 1. unresolvable href is reported, not fatal
const a = new Resvg(doc('company-logo'));
assert.deepEqual(a.pendingImages(), ['company-logo']);
assert.deepEqual(pixel(a), [0, 0, 0, 0], 'nothing drawn yet');

// 2. hand the buffer over -> re-parse -> image drawn
a.resolveImage('company-logo', red);
assert.deepEqual(a.pendingImages(), []);
assert.deepEqual(pixel(a), [255, 0, 0, 255], 'red image drawn');

// 3. preload through the constructor: nothing pending, one parse
const b = new Resvg(doc('company-logo'), null, null, { 'company-logo': red });
assert.deepEqual(b.pendingImages(), []);
assert.deepEqual(pixel(b), [255, 0, 0, 255]);

// 4. regression: real paths still resolve off disk via resourcesDir
const dir = mkdtempSync(join(tmpdir(), 'resvg-'));
writeFileSync(join(dir, 'disk.png'), red);
const c = new Resvg(doc('disk.png'), { resourcesDir: dir });
assert.deepEqual(c.pendingImages(), []);
assert.deepEqual(pixel(c), [255, 0, 0, 255], 'loaded from disk');

// 5. regression: data: URIs still handled by the usvg default resolver
const d = new Resvg(doc(`data:image/png;base64,${red.toString('base64')}`));
assert.deepEqual(d.pendingImages(), []);
assert.deepEqual(pixel(d), [255, 0, 0, 255], 'data URI');

// 6. nested SVG passed as a buffer is sniffed too
const e = new Resvg(doc('sub'), null, null, {
  sub: Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="#00ff00"/></svg>'),
});
assert.deepEqual(e.pendingImages(), []);
assert.deepEqual(pixel(e), [0, 255, 0, 255], 'nested SVG drawn');

console.log('ok — image resolution: all checks passed');
