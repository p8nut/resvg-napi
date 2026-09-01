// background (RenderParams) and toString (WriteOptions, AST-mapped).
import assert from 'node:assert/strict';
import { testFonts, skip } from './support.mjs';
import { createRequire } from 'node:module';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('../index.js');
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds.
const fontsFound = testFonts(FontDatabase);
if (!fontsFound) {
  skip('output shapes', 'no font in this environment');
  process.exit(0);
}
const { db, family } = fontsFound;
const opts = { fontFamily: family };


const px = (r, p) => [...r.renderRaw(p).data.subarray(0, 4)];
const near = (got, want, tol = 2) =>
  assert.ok(got.every((v, i) => Math.abs(v - want[i]) <= tol), `${got} ≉ ${want}`);

const blank = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"/>';
const red = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#f00"/></svg>';

// --- background --------------------------------------------------------------
// 1. nothing drawn = transparent, unless a background is asked for
assert.deepEqual(px(new Resvg(blank)), [0, 0, 0, 0]);
assert.deepEqual(px(new Resvg(blank), { background: 'teal' }), [0, 128, 128, 255]);

// 2. every CSS3 form svgtypes handles
assert.deepEqual(px(new Resvg(blank), { background: '#eee' }), [238, 238, 238, 255]);
assert.deepEqual(px(new Resvg(blank), { background: 'rgb(1,2,3)' }), [1, 2, 3, 255]);
near(px(new Resvg(blank), { background: 'rgba(255, 0, 0, .5)' }), [255, 0, 0, 128]);
assert.deepEqual(px(new Resvg(blank), { background: 'hsl(120, 100%, 50%)' }), [0, 255, 0, 255]);

// 3. it is a background, not an overlay
assert.deepEqual(px(new Resvg(red), { background: 'teal' }), [255, 0, 0, 255]);

// 4. bad colour is an error, not a silent transparent
assert.throws(() => new Resvg(blank).renderPng({ background: 'not-a-colour' }), /invalid background/);

// 5. and it works through the async path
assert.ok((await new Resvg(blank).renderPngAsync({ background: 'teal' })).length > 0);

// --- toString ----------------------------------------------------------------
const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="60" height="30">
  <defs><linearGradient id="grad"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>
  <rect x="1.23456789" width="58" height="28" fill="url(#grad)"/>
  <text x="5" y="20" font-family="${family}" font-size="10">hi</text>
</svg>`;
const doc = new Resvg(svg, opts, db);

// 6. usvg-simplified output: text became paths
const out = doc.toString();
assert.match(out, /<svg width="60" height="30"/);
assert.match(out, /<path /);
assert.doesNotMatch(out, /<text/, 'text flattened to paths by default');

// 7. every mapped WriteOptions field bites
assert.match(doc.toString({ preserveText: true }), /<text/);
assert.match(doc.toString({ idPrefix: 'x-' }), /id="x-grad"/);
assert.match(doc.toString({ useSingleQuote: true }), /<svg width='60'/);
assert.match(doc.toString({ transformsPrecision: 2 }), /matrix\(58 0 0 28 1\.23 0\)/);
assert.match(doc.toString({ transformsPrecision: 8 }), /matrix\(58 0 0 28 1\.2345679 0\)/);

// 8. precision is clamped to usvg's POW_VEC bound (12): no wrap, no panic.
//    Unclamped, 255 indexes a 13-entry table out of bounds and aborts the process.
assert.equal(doc.toString({ transformsPrecision: 300 }), doc.toString({ transformsPrecision: 12 }));
assert.equal(doc.toString({ coordinatesPrecision: 999 }), doc.toString({ coordinatesPrecision: 12 }));
assert.notEqual(doc.toString({ transformsPrecision: 300 }), doc.toString({ transformsPrecision: 2 }));
//    and the same proof for the other one: without it, an ignored
//    `coordinatesPrecision` would make the clamp assertion above pass vacuously,
//    both sides rendering at the default.
assert.notEqual(
  doc.toString({ coordinatesPrecision: 12 }),
  doc.toString({ coordinatesPrecision: 1 }),
  'coordinatesPrecision is honoured at all',
);

// 9. round trip: the output re-parses to the same pixels
const again = new Resvg(doc.toString(), opts, db);
assert.deepEqual([...again.renderRaw().data], [...doc.renderRaw().data], 'lossless round trip');

console.log('ok — background + toString: all checks passed');
