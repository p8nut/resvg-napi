// Smoke check: fails loudly if the generated bindings break.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { testFonts, skip } from './support.mjs';
const { Resvg, FontDatabase, ShapeRendering } = createRequire(import.meta.url)('../index.js');

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
  // Neither half of this message is portable. Three environments, three
  // answers for the same ENOENT:
  //
  //   linux native   No such file or directory (os error 2)
  //   windows native The system cannot find the file specified. (os error 2)
  //   wasm32-wasip1  No such file or directory (os error 44)
  //
  // The text differs, and so does the number -- wasi-libc numbers ENOENT 44.
  // What holds everywhere is that the failure came from the OS rather than
  // from the font parser, which is all this check is for.
  assert.throws(() => fonts.db.loadFontFile('/nope.ttf'), /os error \d+/);
} else {
  skip('text rendering', 'no font in this environment');
}

// 6. errors surface as JS exceptions
assert.throws(() => new Resvg('<svg'), /invalid SVG/);

// A requested dimension comes back exactly, at every size and every ratio.
//
// It used to not: the scale was an f32 round trip, `(base * (w / base)).ceil()`,
// which lands a hair above the integer often enough that seventeen of the first
// four hundred widths came back one pixel too wide. Every width in this suite was
// an exact multiple of the document's own, so nothing could see it. This sweeps
// instead of picking.
for (const [w, h] of [[100, 50], [100, 33], [37, 91], [1, 1]]) {
  const d = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="red"/></svg>`);
  const wrong = [];
  for (let n = 1; n <= 300; n++) {
    if (d.renderRaw({ width: n }).width !== n) wrong.push(`width:${n}`);
    if (d.renderRaw({ height: n }).height !== n) wrong.push(`height:${n}`);
  }
  assert.deepEqual(wrong, [], `${w}x${h} honours every requested dimension`);
}

// and the PNG header agrees with the pixels
{
  const d = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>');
  const png = d.renderPng({ width: 120 });
  assert.equal(png.readUInt32BE(16), 120, 'IHDR width');
  assert.equal(png.readUInt32BE(20), 60, 'IHDR height, from the ratio and not an f32 detour');
}

console.log('ok — all checks passed');
