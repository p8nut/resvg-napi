// The shape of a path node: geometry, fill, stroke. All of it derived -- these
// types are emitted because `SvgNode.path()` hands one out, and the mapper
// prunes whatever no exposed method returns.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg } = createRequire(import.meta.url)('../index.js');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">
  <defs>
    <linearGradient id="grad"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
    <linearGradient id="one-stop"><stop offset="0" stop-color="lime"/></linearGradient>
    <pattern id="pat" width="4" height="4"><rect width="2" height="2"/></pattern>
  </defs>
  <path id="tri" d="M10 10 L50 10 Q60 30 30 50 Z" fill="#38bdf8"
        stroke="teal" stroke-width="3" stroke-dasharray="4 2" stroke-dashoffset="1"/>
  <rect id="grad-filled" x="2" y="55" width="20" height="20" fill="url(#grad)"/>
  <rect id="pat-filled"  x="30" y="55" width="20" height="20" fill="url(#pat)"/>
  <rect id="one-stop-filled" x="60" y="55" width="20" height="20" fill="url(#one-stop)"/>
  <rect id="unfilled" x="60" y="10" width="20" height="20" fill="none"/>
  <g id="grp"><rect x="0" y="0" width="5" height="5"/></g>
</svg>`;

const doc = new Resvg(svg);
const shape = (id) => doc.node(id).path();

// 1. geometry, in the document's own units, with the arity `points` promises
{
  const { data } = shape('tri');
  assert.deepEqual(
    data.map((s) => s.type),
    ['moveTo', 'lineTo', 'quadTo', 'close'],
  );
  assert.deepEqual(data.map((s) => s.points.length), [2, 2, 4, 0],
    'one point for moveTo/lineTo, two for quadTo, none for close');
  assert.deepEqual(data[0].points, [10, 10]);
  assert.deepEqual(data[2].points, [60, 30, 30, 50], 'control then end');
}

// 2. fill: a colour arrives resolved and tagged
{
  const { fill } = shape('tri');
  assert.equal(fill.paint.type, 'color');
  assert.deepEqual(fill.paint.color, { red: 56, green: 189, blue: 248 });
  assert.equal(fill.opacity, 1);
  assert.equal(fill.rule, 'nonZero');
}

// 3. stroke carries its own paint plus the dash pattern
{
  const { stroke } = shape('tri');
  assert.equal(stroke.paint.type, 'color');
  assert.deepEqual(stroke.paint.color, { red: 0, green: 128, blue: 128 });
  assert.equal(stroke.width, 3);
  assert.deepEqual(stroke.dasharray, [4, 2]);
  assert.equal(stroke.dashoffset, 1);
}

// 4. a paint server arrives as an id, not a copy: the document shares one
//    gradient between every element referencing it. `id` is a field here and a
//    method on the wrapper classes -- objects get fields, classes get accessors.
{
  const g = shape('grad-filled').fill.paint;
  assert.equal(g.type, 'linearGradient');
  assert.equal(g.id, 'grad');
  assert.equal(g.color, undefined, 'a server carries no colour');
  assert.ok(doc.linearGradients().some((x) => x.id() === g.id), 'id resolves in defs');

  const p = shape('pat-filled').fill.paint;
  assert.equal(p.type, 'pattern');
  assert.ok(doc.patterns().some((x) => x.id() === p.id));
}

// 5. usvg normalises a single-stop gradient into the plain colour it is
//    equivalent to, per the spec. The reference is gone before it reaches us --
//    worth pinning, it looks like a bug otherwise.
{
  const { paint } = shape('one-stop-filled').fill;
  assert.equal(paint.type, 'color', 'one stop collapses to a colour');
  assert.deepEqual(paint.color, { red: 0, green: 255, blue: 0 });
}

// 6. absent is absent: no fill, no stroke, and not a shape at all
assert.equal(shape('unfilled').fill, undefined, 'fill="none" has no fill');
assert.equal(shape('tri').stroke !== undefined, true);
assert.equal(shape('grad-filled').stroke, undefined, 'filled but not stroked');
assert.equal(doc.node('grp').path(), null, 'a group is not a shape');

// 7. the boxes agree with what SvgNode already reported, so `path()` is a view
//    of the same node rather than a second source of truth
{
  const n = doc.node('tri');
  assert.deepEqual(n.path().boundingBox, n.boundingBox());
}

console.log('ok — path shape: all checks passed');
