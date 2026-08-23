// Horizontal fitting driven by the template's own `data-maxwidth`.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fitTextWidths, findWidthConstraints } from './fit.mjs';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('./index.js');

const fonts = new FontDatabase();
fonts.loadFontData(readFileSync('/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf'));
const family = fonts.faces()[0].families[0];
fonts.setSansSerifFamily(family);
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

// 5. a constraint with no id cannot be measured, and says so
const noId = fitTextWidths(
  '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="30">'
  + '<text x="2" y="20" font-size="12" data-maxwidth="10">overflowing</text></svg>', render);
assert.match(noId.problems[0], /no id, so it cannot be measured/);

// 6. an element with no visible extent is reported, not silently skipped
const empty = fitTextWidths(
  '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="30">'
  + '<text id="blank" x="2" y="20" font-size="12" data-maxwidth="10"></text></svg>', render);
assert.match(empty.problems[0], /no visible extent/);

// 7. a transform is created when the element has none
const bare = fitTextWidths(
  '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="30">'
  + '<text id="bare" x="2" y="20" font-size="14" data-maxwidth="20">far too long for that</text></svg>', render);
assert.match(bare.svg, /transform="scale\(0\.\d+ 1\)"/);
assert.ok(Math.abs(bare.adjustments[0].measured - 20) < 0.5);

console.log('ok — horizontal fitting: all checks passed');
