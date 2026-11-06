// The laid-out content of a text node, `usvg.Text`: chunks, spans, the
// resolved layout and the outlines (the basics: test/api/nodes.mjs).
import assert from 'node:assert/strict';
import { fontsOrSkip, renderNodePng } from './support.mjs';
import binding from '../index.js';
const { usvg } = binding;

const { db, family } = fontsOrSkip(binding, 'text content');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="220" height="90">
  <text id="one" x="10" y="30" font-size="20" fill="#38bdf8">Hi there</text>
  <text id="anchored" x="100" y="60" font-size="12" text-anchor="middle">mid</text>
  <text id="split" x="10" y="80" font-size="12">plain<tspan fill="teal" font-size="18">loud</tspan></text>
  <text id="deco" x="10" y="88" font-size="12" fill="#c2410c"
        text-decoration="underline line-through">marked</text>
  <rect id="notext" width="5" height="5"/>
</svg>`;

const tree = usvg.Tree.fromStr(svg, { fontdb: db, fontFamily: family });
const text = (id) => tree.nodeById(id).value;

// 1. a rect is a path node, not a text one
assert.equal(tree.nodeById('notext').kind, 'path');
assert.equal(tree.nodeById('one').kind, 'text');

// 2. a chunk keeps the source string and the anchor it was given
{
  const [c] = text('one').chunks();
  assert.equal(c.x, 10);
  assert.equal(text('anchored').chunks()[0].anchor, 'middle');
}

// 3. a tspan is its own span, with its own size and fill, over a range of the
//    chunk's text
{
  const [c] = text('split').chunks();
  assert.equal(c.text, 'plainloud');
  assert.equal(c.spans.length, 2, 'the tspan splits the chunk');
  const [plain, loud] = c.spans;
  assert.deepEqual([plain.start, plain.end], [0, 5]);
  assert.deepEqual([loud.start, loud.end], [5, 9]);
  assert.deepEqual(loud.fill.paint.value, { red: 0, green: 128, blue: 128 });
}

// 4. `layouted` is the resolved side: one span here, at the size asked for
{
  const layouted = text('one').layouted();
  assert.equal(layouted.length, 1);
  assert.equal(layouted[0].fontSize, 20);
}

// 5. baselineShift is a derived discriminated union: usvg's BaselineShift is a
//    payload enum, and the generator emits one union per such enum rather
//    than dropping the member. The unit variants share a struct, the one that
//    carries a value gets its own.
{
  const span = text('one').chunks()[0].spans[0];
  assert.ok(Array.isArray(span.baselineShift));
  for (const b of span.baselineShift) {
    assert.match(b.type, /^(baseline|subscript|superscript|number)$/);
    // only the number variant carries a value
    assert.equal('value' in b, b.type === 'number');
  }
}

// 6. the fill reaches through to the paint union, same shape as a path's
assert.equal(text('one').chunks()[0].spans[0].fill.paint.type, 'color');

// 7. text-decoration reaches through as its own object, one entry per line the
//    document asked for, each carrying the paint it is drawn with. usvg keeps
//    these behind accessors on a struct whose fields are pub(crate), so nothing
//    here is a field read -- it is the generator following methods.
{
  const plain = text('one').chunks()[0].spans[0].decoration;
  assert.equal(plain.underline, undefined, 'undecorated text carries no lines');
  assert.equal(plain.overline, undefined);
  assert.equal(plain.lineThrough, undefined);

  const marked = text('deco').chunks()[0].spans[0].decoration;
  assert.ok(marked.underline, 'underline was asked for');
  assert.ok(marked.lineThrough, 'line-through was asked for');
  assert.equal(marked.overline, undefined, 'overline was not');
  // the line inherits the text's own paint, and it is a full Fill
  assert.equal(marked.underline.fill.paint.type, 'color');
  assert.deepEqual(
    [marked.underline.fill.paint.value.red, marked.underline.fill.paint.value.green],
    [0xc2, 0x41],
  );
}

// 8. a laid-out span's lines are whole paths, not the string napi would make
//    of a type *named* `Path`, and null when the span has none.
{
  const sp = text('deco').layouted()[0];
  assert.ok(sp.underline instanceof usvg.Path, 'a usvg.Path');
  assert.ok(sp.underline.data().length > 0 && sp.underline.fill(), 'a whole Path');
  assert.equal(text('one').layouted()[0].underline, null, 'and null when the span has none');
}

// the glyphs as the paths a render draws: reachable as nodes, still filled
// with the text's own paint, and drawable on their own
{
  const flat = text('one').flattened().children();
  assert.ok(flat.length > 0);
  const shapes = flat.flatMap(function leaves(n) {
    return n.kind === 'group' ? n.value.children().flatMap(leaves) : [n];
  });
  assert.ok(shapes.every((n) => n.kind === 'path' && n.value.data().length > 0));
  assert.equal(shapes[0].value.fill().paint.type, 'color');
  assert.ok(renderNodePng(binding, flat[0]).length > 0);
  assert.equal(tree.nodeById('notext').value.flattened, undefined, 'a path has no outlines to flatten');
}
