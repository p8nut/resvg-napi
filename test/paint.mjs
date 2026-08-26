// Fill and stroke paint, as a discriminated union: `usvg::Paint` is an enum
// carrying a payload, so the template splits it by hand.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg } = createRequire(import.meta.url)('../index.js');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">
  <defs>
    <linearGradient id="grad"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
    <linearGradient id="one-stop"><stop offset="0" stop-color="lime"/></linearGradient>
    <pattern id="pat" width="4" height="4"><rect width="2" height="2"/></pattern>
  </defs>
  <rect id="solid" x="2" y="2"  width="40" height="20" fill="#38bdf8" stroke="teal" stroke-width="2"/>
  <rect id="grad-filled" x="2" y="30" width="40" height="20" fill="url(#grad)"/>
  <rect id="pat-filled"  x="2" y="55" width="40" height="20" fill="url(#pat)"/>
  <rect id="one-stop-filled" x="50" y="30" width="40" height="20" fill="url(#one-stop)"/>
  <rect id="unfilled" x="50" y="2" width="40" height="20" fill="none"/>
  <g id="grp"><rect x="60" y="40" width="10" height="10"/></g>
</svg>`;

const doc = new Resvg(svg);
const paint = (id) => doc.node(id).fillPaint();

// 1. a colour arrives resolved, tagged, with no server fields
{
  const p = paint('solid');
  assert.equal(p.type, 'color');
  assert.deepEqual(p.color, { red: 56, green: 189, blue: 248 });
  assert.equal(p.id, undefined, 'a colour carries no server id');
}

// 2. a paint server arrives as an id, not a copy: the document shares one
//    gradient between every element that references it.
{
  const g = paint('grad-filled');
  assert.equal(g.type, 'linearGradient');
  assert.equal(g.id, 'grad');
  assert.equal(g.color, undefined, 'a server carries no colour');
  // and the id resolves against the def table
  // `id` is a property on PaintServer but a method on the generated classes:
  // objects get fields, wrapper classes get accessors.
  assert.ok(doc.linearGradients().some((x) => x.id() === g.id), 'id resolves in defs');

  assert.equal(paint('pat-filled').type, 'pattern');
  assert.ok(doc.patterns().some((x) => x.id() === paint('pat-filled').id));
}

// 3. usvg normalises a single-stop gradient into the plain colour it is
//    equivalent to, per the SVG spec. So the reference disappears before it
//    ever reaches us -- worth pinning, it looks like a bug otherwise.
{
  const p = paint('one-stop-filled');
  assert.equal(p.type, 'color', 'one stop collapses to a colour');
  assert.deepEqual(p.color, { red: 0, green: 255, blue: 0 });
}

// 4. null is the answer for "no paint", distinct from "not a shape"
assert.equal(paint('unfilled'), null, 'fill="none" has no paint');
assert.equal(paint('grp'), null, 'a group is not a shape');

// 5. stroke follows the same shape, and is independent of fill
{
  const s = doc.node('solid').strokePaint();
  assert.equal(s.type, 'color');
  assert.deepEqual(s.color, { red: 0, green: 128, blue: 128 });
  assert.equal(doc.node('grad-filled').strokePaint(), null, 'filled but not stroked');
}

console.log('ok — fill/stroke paint: all checks passed');
