// The laid-out content of a text node. These types exist because
// `SvgNode.text()` hands one out: the mapper prunes whatever no exposed method
// returns, and a node payload is not a collection, so nothing else nominates
// them.
import assert from 'node:assert/strict';
import { testFonts, skip } from './support.mjs';
import { createRequire } from 'node:module';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('../index.js');

const fontsFound = testFonts(FontDatabase);
if (!fontsFound) {
  skip('text content', 'no font in this environment');
  process.exit(0);
}
const { db, family } = fontsFound;

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="220" height="90">
  <text id="one" x="10" y="30" font-size="20" fill="#38bdf8">Hi there</text>
  <text id="anchored" x="100" y="60" font-size="12" text-anchor="middle">mid</text>
  <text id="split" x="10" y="80" font-size="12">plain<tspan fill="teal" font-size="18">loud</tspan></text>
  <rect id="notext" width="5" height="5"/>
</svg>`;

const doc = new Resvg(svg, { fontFamily: family }, db);
const text = (id) => doc.node(id).text();

// 1. not a text node is null, distinct from an empty one
assert.equal(doc.node('notext').text(), null, 'a rect is not text');

// 2. a chunk keeps the source string and the anchor it was given
{
  const t = text('one');
  assert.equal(t.chunks.length, 1);
  const c = t.chunks[0];
  assert.equal(c.text, 'Hi there');
  assert.equal(c.x, 10);
  assert.equal(c.anchor, 'start');
  assert.equal(text('anchored').chunks[0].anchor, 'middle');
}

// 3. a tspan is its own span, with its own size and fill, over a range of the
//    chunk's text
{
  const c = text('split').chunks[0];
  assert.equal(c.text, 'plainloud');
  assert.equal(c.spans.length, 2, 'the tspan splits the chunk');
  const [plain, loud] = c.spans;
  assert.deepEqual([plain.start, plain.end], [0, 5]);
  assert.deepEqual([loud.start, loud.end], [5, 9]);
  assert.equal(plain.fontSize, 12);
  assert.equal(loud.fontSize, 18);
  assert.deepEqual(loud.fill.paint.value, { red: 0, green: 128, blue: 128 });
}

// 4. `layouted` is the resolved side: positioned glyphs, one per character here
{
  const t = text('one');
  assert.equal(t.layouted.length, 1);
  const span = t.layouted[0];
  assert.equal(span.fontSize, 20);
  assert.equal(span.positionedGlyphs.length, 'Hi there'.length,
    'one glyph per character for this string');
}

// 5. baselineShift is a derived discriminated union: usvg's BaselineShift is a
//    payload enum, and the generator now emits one union per such enum rather
//    than dropping the member. The unit variants share a struct, the one that
//    carries a value gets its own.
{
  const span = text('one').chunks[0].spans[0];
  assert.ok(Array.isArray(span.baselineShift));
  for (const b of span.baselineShift) {
    assert.match(b.type, /^(baseline|subscript|superscript|number)$/);
    // only the number variant carries a value
    assert.equal('value' in b, b.type === 'number');
  }
}

// 6. the fill reaches through to the paint union, same shape as a path's
assert.equal(text('one').chunks[0].spans[0].fill.paint.type, 'color');

// 7. the boxes agree with what SvgNode reports, so text() is a view of the same
//    node rather than a second source of truth
assert.deepEqual(text('one').boundingBox, doc.node('one').boundingBox());

console.log('ok — text content: all checks passed');
