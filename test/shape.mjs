// The shape of a path node, `usvg.Path`: geometry, fill, stroke. All of it
// derived -- the classes are generated from usvg's own types.
import assert from 'node:assert/strict';
import binding from '../index.js';
const { usvg } = binding;

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

const tree = usvg.Tree.fromStr(svg);
const shape = (id) => tree.nodeById(id).value;

// 1. geometry, in the document's own units: what `tiny_skia_path::Path::
//    segments()` iterates, `PathSegment` as a union. One point for
//    moveTo/lineTo, a tuple of them for quadTo/cubicTo (`[control, end]`),
//    none for close; each point a `tiny_skia.PointObject`.
{
  const data = shape('tri').data();
  assert.deepEqual(data, [
    { type: 'moveTo', value: { x: 10, y: 10 } },
    { type: 'lineTo', value: { x: 50, y: 10 } },
    { type: 'quadTo', value: [{ x: 60, y: 30 }, { x: 30, y: 50 }] },
    { type: 'close' },
  ]);
  // A point builds back into the class.
  assert.equal(binding.tiny_skia.Point.fromObject(data[2].value[1]).x, 30);
  const curve = usvg.Tree.fromStr('<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><path id="c" d="M0 0 C1 2 3 4 5 6" stroke="red"/></svg>');
  assert.deepEqual(curve.nodeById('c').value.data()[1],
    { type: 'cubicTo', value: [{ x: 1, y: 2 }, { x: 3, y: 4 }, { x: 5, y: 6 }] });
}

// 2. fill: a colour arrives resolved and tagged
{
  const fill = shape('tri').fill();
  assert.equal(fill.paint.type, 'color');
  assert.deepEqual(fill.paint.value, { red: 56, green: 189, blue: 248 });
  assert.equal(fill.opacity, 1);
  assert.equal(fill.rule, 'nonZero');
}

// 3. stroke carries its own paint plus the dash pattern
{
  const stroke = shape('tri').stroke();
  assert.equal(stroke.paint.type, 'color');
  assert.deepEqual(stroke.paint.value, { red: 0, green: 128, blue: 128 });
  assert.equal(stroke.width, 3);
  assert.deepEqual(stroke.dasharray, [4, 2]);
  assert.equal(stroke.dashoffset, 1);
}

// 4. a paint server arrives as an id, not a copy: the document shares one
//    gradient between every element referencing it. `id` is a field here and a
//    method on the classes -- objects get fields, classes get accessors.
{
  const g = shape('grad-filled').fill().paint;
  assert.equal(g.type, 'linearGradient');
  assert.equal(g.id, 'grad');
  assert.equal(g.value, undefined, 'a server carries no value, only an id');
  assert.ok(tree.linearGradients().some((x) => x.id() === g.id), 'id resolves in defs');

  const p = shape('pat-filled').fill().paint;
  assert.equal(p.type, 'pattern');
  assert.ok(tree.patterns().some((x) => x.id() === p.id));
}

// 5. usvg normalises a single-stop gradient into the plain colour it is
//    equivalent to, per the spec. The reference is gone before it reaches us --
//    worth pinning, it looks like a bug otherwise.
{
  const { paint } = shape('one-stop-filled').fill();
  assert.equal(paint.type, 'color', 'one stop collapses to a colour');
  assert.deepEqual(paint.value, { red: 0, green: 255, blue: 0 });
}

// 6. absent is absent: no fill, no stroke, and not a shape at all
assert.equal(shape('unfilled').fill(), null, 'fill="none" has no fill');
assert.notEqual(shape('tri').stroke(), null);
assert.equal(shape('grad-filled').stroke(), null, 'filled but not stroked');
assert.equal(tree.nodeById('grp').kind, 'group', 'a group is not a shape');

// A group's own properties, read off the group: a path has none of them.
{
  const t = usvg.Tree.fromStr(
    '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">' +
    '<filter id="f"><feOffset dx="1"/></filter>' +
    '<g id="g" transform="translate(3 4)" opacity="0.5" filter="url(#f)"' +
    ' style="mix-blend-mode:multiply;isolation:isolate"><rect id="r" width="5" height="5"/></g></svg>',
  );
  const g = t.nodeById('g').value;
  assert.deepEqual([g.transform().tx, g.transform().ty], [3, 4]);
  assert.equal(g.opacity(), 0.5);
  assert.equal(g.blendMode(), 'multiply');
  assert.equal(g.isolate(), true);
  assert.equal(g.shouldIsolate(), true);
  assert.deepEqual(g.filters().map((f) => f.id()), ['f']);
  assert.equal(g.hasChildren(), true);
  assert.ok(g.layerBoundingBox().width() > 0);

  const rect = t.nodeById('r').value;
  for (const m of ['transform', 'opacity', 'blendMode', 'isolate', 'filters', 'layerBoundingBox', 'hasChildren']) {
    assert.equal(rect[m], undefined, `${m} on a path`);
  }
}
