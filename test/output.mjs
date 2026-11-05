// The helpers' `background`, and `usvg.Tree.toString` with `usvg.WriteOptions`.
import assert from 'node:assert/strict';
import { fontsOrSkip } from './support.mjs';
import binding from '../index.js';
import { parse, renderPngAsync, renderRaw } from '../helpers.mjs';
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds.
const { db, family } = fontsOrSkip(binding, 'output shapes');
const opts = { fonts: db, fontFamily: family };

/** A parsed document: the tree the helpers render and `toString` writes. */
const doc = (svg, o = opts) => parse(svg, o).tree;
const px = (tree, p) => [...renderRaw(tree, p).data.subarray(0, 4)];
const near = (got, want, tol = 2) =>
  assert.ok(got.every((v, i) => Math.abs(v - want[i]) <= tol), `${got} ≉ ${want}`);

const blank = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"/>';
const red = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#f00"/></svg>';

// --- background --------------------------------------------------------------
// 1. nothing drawn = transparent, unless a background is asked for
assert.deepEqual(px(doc(blank)), [0, 0, 0, 0]);
assert.deepEqual(px(doc(blank), { background: 'teal' }), [0, 128, 128, 255]);

// 2. every CSS3 form svgtypes handles
assert.deepEqual(px(doc(blank), { background: '#eee' }), [238, 238, 238, 255]);
assert.deepEqual(px(doc(blank), { background: 'rgb(1,2,3)' }), [1, 2, 3, 255]);
near(px(doc(blank), { background: 'rgba(255, 0, 0, .5)' }), [255, 0, 0, 128]);
assert.deepEqual(px(doc(blank), { background: 'hsl(120, 100%, 50%)' }), [0, 255, 0, 255]);

// 3. it is a background, not an overlay
assert.deepEqual(px(doc(red), { background: 'teal' }), [255, 0, 0, 255]);

// 4. bad colour is an error, not a silent transparent
assert.throws(() => renderRaw(doc(blank), { background: 'not-a-colour' }), /invalid background/);

// 5. and it works through the async path
assert.ok((await renderPngAsync(doc(blank), { background: 'teal' })).length > 0);

// --- toString ----------------------------------------------------------------
const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="60" height="30">
  <defs><linearGradient id="grad"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>
  <rect x="1.23456789" width="58" height="28" fill="url(#grad)"/>
  <text x="5" y="20" font-family="${family}" font-size="10">hi</text>
</svg>`;
const tree = doc(svg);

// 6. usvg-simplified output: text became paths
const out = tree.toString();
assert.match(out, /<svg width="60" height="30"/);
assert.match(out, /<path /);
assert.doesNotMatch(out, /<text/, 'text flattened to paths by default');

// 7. every mapped WriteOptions field bites
assert.match(tree.toString({ preserveText: true }), /<text/);
assert.match(tree.toString({ idPrefix: 'x-' }), /id="x-grad"/);
assert.match(tree.toString({ useSingleQuote: true }), /<svg width='60'/);
assert.match(tree.toString({ transformsPrecision: 2 }), /matrix\(58 0 0 28 1\.23 0\)/);
assert.match(tree.toString({ transformsPrecision: 8 }), /matrix\(58 0 0 28 1\.2345679 0\)/);

// 7b. indentation, xmlwriter's `Indent`: spaces, tabs or none
assert.match(tree.toString(), /\n    <defs>/, 'default: four spaces');
assert.match(tree.toString({ indent: 2 }), /\n  <defs>/);
assert.match(tree.toString({ indent: 'tabs' }), /\n\t<defs>/);
assert.ok(!tree.toString({ indent: 'none' }).includes('\n'), 'none: a single line');
assert.match(tree.toString({ attributesIndent: 'tabs' }), /<svg\n\twidth=/);
assert.throws(() => tree.toString({ indent: 256 }), /invalid indent: 256/);
assert.throws(() => tree.toString({ attributesIndent: 'wide' }), /invalid attributesIndent: "wide"/);

// 8. precision is clamped to usvg's POW_VEC bound (12): no wrap, no panic.
//    Unclamped, 255 indexes a 13-entry table out of bounds and aborts the process.
assert.equal(tree.toString({ transformsPrecision: 300 }), tree.toString({ transformsPrecision: 12 }));
assert.equal(tree.toString({ coordinatesPrecision: 999 }), tree.toString({ coordinatesPrecision: 12 }));
assert.notEqual(tree.toString({ transformsPrecision: 300 }), tree.toString({ transformsPrecision: 2 }));
//    and the same proof for the other one: without it, an ignored
//    `coordinatesPrecision` would make the clamp assertion above pass vacuously,
//    both sides rendering at the default.
assert.notEqual(
  tree.toString({ coordinatesPrecision: 12 }),
  tree.toString({ coordinatesPrecision: 1 }),
  'coordinatesPrecision is honoured at all',
);

// 9. round trip: the output re-parses to the same pixels
const again = doc(tree.toString());
assert.deepEqual([...renderRaw(again).data], [...renderRaw(tree).data], 'lossless round trip');
