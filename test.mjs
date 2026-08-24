// Smoke check: fails loudly if the generated bindings break.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { testFonts, skip } from './test-support.mjs';
const { Resvg, FontDatabase, ShapeRendering } = createRequire(import.meta.url)('./index.js');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
  <rect width="100" height="50" fill="#3b82f6"/>
  <text x="10" y="30" font-size="16" fill="white">hi</text>
</svg>`;

// 1. string input, default options
const r = new Resvg(svg);
assert.equal(r.width, 100);
assert.equal(r.height, 50);

// 2. PNG buffer
const png = r.renderPng();
assert.deepEqual([...png.subarray(0, 4)], [0x89, 0x50, 0x4e, 0x47], 'PNG magic');

// 3. scaling + raw RGBA
const raw = r.renderRaw({ width: 200 });
assert.equal(raw.width, 200);
assert.equal(raw.height, 100);
assert.equal(raw.data.length, 200 * 100 * 4);
assert.deepEqual([...raw.data.subarray(0, 4)], [0x3b, 0x82, 0xf6, 0xff], 'top-left pixel = #3b82f6');

// 4. AST-generated options object (flat JSON) + string enum
const r2 = new Resvg(Buffer.from(svg), {
  dpi: 192,
  fontFamily: 'DejaVu Sans',
  fontSize: 20,
  languages: ['fr'],
  shapeRendering: ShapeRendering.CrispEdges,
  defaultSizeWidth: 300,
  defaultSizeHeight: 300,
  resourcesDir: process.cwd(),
});
assert.equal(r2.width, 100);

// 5. opaque fontdb class
assert.equal(new FontDatabase().len(), 0, 'a fresh database is empty');
const fonts = testFonts(FontDatabase);
if (fonts) {
  assert.ok(fonts.db.len() > 0);
  assert.ok(new Resvg(svg, { fontFamily: fonts.family }, fonts.db).renderPng().length > 0);
  assert.throws(() => fonts.db.loadFontFile('/nope.ttf'), /No such file|nope/);
} else {
  skip('text rendering', 'no font in this environment');
}

// 6. errors surface as JS exceptions
assert.throws(() => new Resvg('<svg'), /invalid SVG/);

console.log('ok — all checks passed');
