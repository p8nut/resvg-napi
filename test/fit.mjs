// Horizontal fitting driven by the template's own `data-maxwidth`.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { testFonts, skip } from './support.mjs';
import { fitTextWidths, findWidthConstraints } from '../fit.mjs';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('../index.js');
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds. testFonts also points the
// generic families at it, so `font-size` on a bare <text> resolves.
const fontsFound = testFonts(FontDatabase);
if (!fontsFound) {
  skip('horizontal fitting', 'no font in this environment');
  process.exit(0);
}
const { db: fonts, family } = fontsFound;
const render = (svg) => new Resvg(svg, { fontFamily: family }, fonts);

// Shaped like a card template: a translate already in place, per-element limits.
const card = (long) => `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="100">
  <text id="short" transform="translate(10 30)" x="0" y="0" font-size="12" data-maxwidth="200">Ada</text>
  <text id="long" transform="translate(10 60) scale(1.11 1)" x="0" y="0" font-size="12" data-maxwidth="80">${long}</text>
  <text id="nolimit" transform="translate(10 90)" x="0" y="0" font-size="12">unconstrained</text>
</svg>`;

// 1. the constraints come from the document, not from the code
const found = findWidthConstraints(card('x'));
assert.deepEqual(found.map((c) => [c.id, c.max]), [['short', 200], ['long', 80]]);

// 2. a name that overflows is compressed to exactly its limit
const svg = card('Wolfeschlegelsteinhausen');
const before = render(svg).node('long').extent().width;
assert.ok(before > 80, `starts too wide: ${before}`);
const { svg: fitted, adjustments, problems } = fitTextWidths(svg, render);
assert.deepEqual(problems, []);
assert.deepEqual(adjustments.map((a) => a.id), ['long'], 'only the overflowing one moved');
const [adj] = adjustments;
assert.ok(adj.factor < 1 && adj.factor > 0.1, `factor ${adj.factor}`);
assert.ok(Math.abs(adj.measured - 80) < 0.5, `landed at ${adj.measured}, wanted 80`);

// 3. the existing transform is kept, the scale composed onto it
assert.match(fitted, /transform="translate\(10 60\) scale\(1\.11 1\) scale\(0\.\d+ 1\)"/);
assert.match(fitted, /id="short" transform="translate\(10 30\)"/, 'untouched element unchanged');

// 4. text that already fits is left alone, and so is the unconstrained one
const roomy = fitTextWidths(card('Ada'), render);
assert.deepEqual(roomy.adjustments, []);
assert.equal(roomy.svg, card('Ada'));

// 5. an element without an id is still fitted: the constraint should not depend
//    on the template carrying ids, so one is generated
const noId = fitTextWidths(
  '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="30">'
  + '<text x="2" y="20" font-size="12" data-maxwidth="10">overflowing</text></svg>', render);
assert.deepEqual(noId.problems, []);
assert.equal(noId.adjustments.length, 1);
assert.match(noId.svg, /<text id="fit-1"/);
assert.ok(Math.abs(noId.adjustments[0].measured - 10) < 0.5);

// 6. an element with no visible extent is reported, not silently skipped
const empty = fitTextWidths(
  '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="30">'
  + '<text id="blank" x="2" y="20" font-size="12" data-maxwidth="10"></text></svg>', render);
assert.match(empty.problems[0], /no visible extent/);

// 7. a transform is created when the element has none
const bare = fitTextWidths(
  '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="30">'
  + '<text id="bare" x="2" y="20" font-size="14" data-maxwidth="20">far too long for that</text></svg>', render);
// x="2", so the scale is anchored there rather than at the origin
assert.match(bare.svg, /transform="translate\(2 0\) scale\(0\.\d+ 1\) translate\(-2 0\)"/);
assert.ok(Math.abs(bare.adjustments[0].measured - 20) < 0.5);

// 8. the limit is in the document's own units, not the canvas ones. `85.6mm`
//    with a 240.94-wide viewBox makes usvg normalise the tree to 323.53, so a
//    naive comparison inflates every width by 1.343 and compresses text that fits.
const inMm = (text, max) => `<svg xmlns="http://www.w3.org/2000/svg" width="85.6mm" height="53.98mm"
  preserveAspectRatio="none" viewBox="0 0 240.94 153.07">
  <text id="t" transform="translate(10 30)" x="0" y="0" font-size="12" data-maxwidth="${max}">${text}</text>
</svg>`;
{
  const src = inMm('ab', 15);
  const canvas = render(src).node('t').extent().width;
  assert.ok(canvas > 15, `canvas width ${canvas} exceeds the limit, which is the trap`);
  const { adjustments, problems } = fitTextWidths(src, render);
  assert.deepEqual(problems, []);
  assert.deepEqual(adjustments, [], 'fits in document units, so nothing to do');
}
{
  const { adjustments } = fitTextWidths(inMm('Wolfeschlegelsteinhausen', 60), render);
  assert.equal(adjustments.length, 1);
  assert.ok(Math.abs(adjustments[0].measured - 60) < 0.5,
    `landed at ${adjustments[0].measured} document units, wanted 60`);
}

// 9. an element positioned by `x` must be compressed in place, not slid toward
//    the origin: a bare scale() is anchored at 0 and moves the text.
{
  const src = `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="120">
  <text x="28" y="55" font-size="30" data-maxwidth="10">resvg</text>
  <text x="28" y="85" font-size="30" data-maxwidth="10">resvg</text>
</svg>`;
  const { svg: fitted, adjustments, problems } = fitTextWidths(src, render);
  assert.deepEqual(problems, []);
  assert.equal(adjustments.length, 2, 'both, and neither needed an id in the source');
  const after = render(fitted);
  for (const a of adjustments) {
    const box = after.node(a.id).extent();
    assert.ok(Math.abs(a.measured - 10) < 0.5, `${a.id} width ${a.measured}`);
    // The invariant is the anchor, x=28, not the ink edge: the first glyph's
    // side bearing compresses too, so the ink starts nearer the anchor than
    // before. Anchored at the origin instead, this lands around 4.
    assert.ok(box.x >= 27.9 && box.x < 30,
      `${a.id} ink starts at ${box.x.toFixed(2)}, expected just right of the x=28 anchor`);
  }
  // two near-identical tags must get distinct ids, numbered in document order
  const ids = [...fitted.matchAll(/id="(fit-\d+)"/g)].map((m) => m[1]);
  assert.deepEqual(ids, ['fit-1', 'fit-2']);
}

console.log('ok — horizontal fitting: all checks passed');
