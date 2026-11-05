// The tree's nodes through the namespaced surface (ADR 0002, Q22):
// `tree.root()` is a `usvg.Group`, `group.children()` and `tree.nodeById()`
// hand out `usvg.Node`, a `{ kind, value }` union, and every value is a
// projection: it keeps the tree alive and finds its node again on each call.
// `resvg.renderNode` draws one, the same pixels as the flat `renderNodePng`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';
import binding from '../../index.js';

const { usvg, fontdb, tiny_skia, resvg } = binding;

const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));
const FAMILY = 'IBM Plex Sans';

const SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <filter id="f"><feGaussianBlur stdDeviation="5"/></filter>
  <g id="wrap" filter="url(#f)">
    <rect id="r" x="50" y="50" width="40" height="40" fill="red"/>
  </g>
  <g id="outer"><g id="mid"><rect id="deep" x="5" y="5" width="10" height="10" fill="blue"/></g></g>
</svg>`;

const rect = (r) => [r.x(), r.y(), r.width(), r.height()];
const bbox = (b) => b && [b.x, b.y, b.width, b.height];
const ids = (nodes) => nodes.map(({ kind, value }) => [kind, value.id()]);

test('the root is a group, its children a { kind, value } union', () => {
  const tree = usvg.Tree.fromStr(SVG);
  const root = tree.root();
  assert.ok(root instanceof usvg.Group);
  assert.deepEqual(ids(root.children()), [['group', 'wrap'], ['group', 'outer']]);
  const [wrap] = root.children();
  const [r] = wrap.value.children();
  assert.equal(r.kind, 'path');
  assert.ok(r.value instanceof usvg.Path);
  assert.equal(r.value.children, undefined, 'a path has no children');
  // At depth, and by id from anywhere.
  const deep = root.children()[1].value.children()[0].value.children()[0];
  assert.deepEqual([deep.kind, deep.value.id()], ['path', 'deep']);
  assert.deepEqual(rect(deep.value.absBoundingBox()), [5, 5, 10, 10]);
  assert.deepEqual(rect(tree.nodeById('deep').value.absBoundingBox()), [5, 5, 10, 10]);
  assert.equal(tree.nodeById('nope'), null);
  assert.equal(tree.nodeById('mid').kind, 'group');
});

test('bounding boxes, transforms and the inner group\'s filter extent', () => {
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">
    <g id="g" transform="translate(50 20)">
      <rect id="box" x="10" y="10" width="60" height="40" fill="tomato" stroke="black" stroke-width="8"/>
    </g>
  </svg>`);
  const box = tree.nodeById('box').value;
  assert.deepEqual(rect(box.boundingBox()), [10, 10, 60, 40], 'geometry, in its own space');
  assert.deepEqual(rect(box.absBoundingBox()), [60, 30, 60, 40]);
  assert.deepEqual(rect(box.strokeBoundingBox()), [6, 6, 68, 48], 'stroke widens by 4 a side');
  assert.deepEqual(rect(box.absStrokeBoundingBox()), [56, 26, 68, 48]);
  const g = tree.nodeById('g').value;
  assert.ok(g.transform() instanceof tiny_skia.Transform);
  assert.deepEqual([g.transform().tx, g.transform().ty], [50, 20]);
  assert.deepEqual([box.absTransform().tx, box.absTransform().ty], [50, 20]);
  const root = tree.root();
  assert.deepEqual(bbox(root.absLayerBoundingBox()), [56, 26, 68, 48]);
  assert.equal(root.filtersBoundingBox(), null, 'the root carries no filter');
  assert.equal(root.hasChildren(), true);
  // usvg wraps a filtered element in a group of its own: its extent is seen there.
  const blurred = usvg.Tree.fromStr(SVG);
  const wrap = blurred.nodeById('wrap').value;
  assert.deepEqual(bbox(wrap.filtersBoundingBox()), [46, 46, 48, 48], 'blur spill');
  assert.deepEqual(wrap.filters().map((f) => f.id()), ['f']);
  assert.equal(blurred.nodeById('wrap').value.filtersBoundingBox().width, 48);
});

test('clip path, mask and pattern: projections too, their content a group', () => {
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="60" height="40">
    <defs><clipPath id="cp"><rect id="cpr" width="30" height="40"/></clipPath>
          <clipPath id="outer" clip-path="url(#cp)"><circle cx="20" cy="20" r="10"/></clipPath>
          <mask id="mk"><rect id="mkr" width="40" height="40" fill="white"/></mask>
          <pattern id="pt" width="10" height="10" patternUnits="userSpaceOnUse">
            <circle id="dot" cx="5" cy="5" r="3" fill="black"/></pattern></defs>
    <g id="a" clip-path="url(#cp)"><rect width="60" height="40" fill="red"/></g>
    <g id="b" mask="url(#mk)"><rect width="60" height="40" fill="blue"/></g>
    <g id="c" clip-path="url(#outer)"><rect width="60" height="40" fill="url(#pt)"/></g>
    <rect id="plain" width="10" height="10"/>
  </svg>`);
  // Reached from the group that refers to it: a step, like `children()`.
  const a = tree.nodeById('a').value;
  const cp = a.clipPath();
  assert.ok(cp instanceof usvg.ClipPath);
  assert.equal(cp.id(), 'cp');
  assert.ok(cp.transform() instanceof tiny_skia.Transform);
  assert.equal(a.mask(), null);
  const content = cp.root();
  assert.ok(content instanceof usvg.Group);
  assert.deepEqual(ids(content.children()), [['path', 'cpr']]);
  // A clip path's own clip path, and a mask's content.
  const outer = tree.nodeById('c').value.clipPath();
  assert.equal(outer.id(), 'outer');
  assert.equal(outer.clipPath().id(), 'cp');
  assert.deepEqual(ids(outer.clipPath().root().children()), [['path', 'cpr']]);
  const mk = tree.nodeById('b').value.mask();
  assert.equal(mk.id(), 'mk');
  assert.equal(mk.kind(), 'luminance');
  assert.equal(mk.mask(), null);
  assert.deepEqual(ids(mk.root().children()), [['path', 'mkr']]);
  // From the tree's tables: the same class, one per element of the table.
  assert.deepEqual(tree.clipPaths().map((c) => c.id()).sort(), ['cp', 'outer']);
  const cpFromTable = tree.clipPaths().find((c) => c.id() === 'cp');
  assert.ok(cpFromTable instanceof usvg.ClipPath);
  assert.deepEqual(ids(cpFromTable.root().children()), [['path', 'cpr']]);
  assert.deepEqual(ids(tree.masks()[0].root().children()), [['path', 'mkr']]);
  const [pt] = tree.patterns();
  assert.ok(pt instanceof usvg.Pattern);
  assert.equal(pt.id(), 'pt');
  assert.deepEqual(ids(pt.root().children()), [['path', 'dot']]);
  assert.equal(tree.nodeById('plain').kind, 'path');
});

test('a definition keeps its tree alive too', async () => {
  setFlagsFromString('--expose-gc');
  const gc = runInNewContext('gc');
  const cp = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
    <clipPath id="cp"><rect id="r" width="5" height="5"/></clipPath>
    <rect width="10" height="10" clip-path="url(#cp)"/></svg>`).clipPaths()[0];
  for (let i = 0; i < 3; i++) {
    gc();
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.deepEqual(ids(cp.root().children()), [['path', 'r']]);
});

test('a text node: its chunks, its layout, and its outlines as a group', () => {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="220" height="90">
    <text id="one" x="10" y="30" font-size="20" fill="#38bdf8">Hi there</text>
    <text id="split" x="10" y="80" font-size="12">plain<tspan fill="teal" font-size="18">loud</tspan></text>
  </svg>`, { fontdb: db, fontFamily: FAMILY });
  const one = tree.nodeById('one');
  assert.equal(one.kind, 'text');
  const t = one.value;
  assert.ok(t instanceof usvg.Text);
  assert.equal(t.chunks().length, 1);
  assert.equal(t.chunks()[0].text, 'Hi there');
  assert.equal(t.chunks()[0].anchor, 'start');
  assert.equal(t.layouted()[0].positionedGlyphs.length, 'Hi there'.length);
  assert.ok(t.absBoundingBox().width() > 0);
  // `flattened()` is a group of paths, reached through the text node.
  const flat = t.flattened();
  assert.ok(flat instanceof usvg.Group);
  assert.ok(flat.children().length > 0);
  assert.ok(flat.children().every(({ kind }) => kind === 'path' || kind === 'group'));
  const [plain, loud] = tree.nodeById('split').value.chunks()[0].spans;
  assert.deepEqual([plain.fontSize, loud.fontSize], [12, 18]);
});

test('an image node, and its kind', () => {
  const dot = new tiny_skia.Pixmap(1, 1);
  dot.fill(tiny_skia.Color.fromRgba8(0, 0, 255, 255));
  const href = `data:image/png;base64,${dot.encodePng().toString('base64')}`;
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
    <image id="i" href="${href}" width="10" height="10"/>
  </svg>`);
  // usvg puts the image in a group of its own, which takes the id.
  const [i] = tree.nodeById('i').value.children();
  assert.equal(i.kind, 'image');
  assert.ok(i.value instanceof usvg.Image);
  assert.equal(i.value.kind().type, 'png');
  assert.deepEqual([i.value.size().width(), i.value.size().height()], [1, 1]);
});

test('a node keeps its tree alive, and finds its node again on each call', async () => {
  setFlagsFromString('--expose-gc');
  const gc = runInNewContext('gc');
  // Nothing holds the tree but the node.
  const mid = usvg.Tree.fromStr(SVG).nodeById('mid').value;
  for (let i = 0; i < 3; i++) {
    gc();
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.equal(mid.id(), 'mid');
  assert.deepEqual(ids(mid.children()), [['path', 'deep']]);
  assert.deepEqual(rect(mid.children()[0].value.absBoundingBox()), [5, 5, 10, 10]);
});

/**
 * A node's PNG, placed as the flat `renderNodePng` places it: `extent` is the
 * pixmap, and `inner` the box `resvg::render_node` offsets the node by (its
 * `Node::abs_layer_bounding_box`), both `[x, y, width, height]`.
 */
function nodePng(node, [x, y, w, h], [ix, iy]) {
  const pixmap = new tiny_skia.Pixmap(Math.ceil(w), Math.ceil(h));
  const transform = tiny_skia.Transform.fromTranslate(-x, -y)
    .preConcat(node.value.absTransform())
    .preTranslate(ix, iy);
  assert.equal(resvg.renderNode(node, transform, pixmap), true);
  return pixmap.encodePng();
}

test('renderNode draws a node: the flat renderNodePng\'s pixels', () => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80">
    <g id="g" transform="translate(10 5)" opacity="0.8">
      <rect id="box" x="10" y="10" width="40" height="30" fill="tomato" stroke="black" stroke-width="4"/>
      <circle cx="70" cy="40" r="15" fill="teal"/>
    </g>
  </svg>`;
  const flat = new binding.Resvg(svg);
  const tree = usvg.Tree.fromStr(svg);
  // A path's extent is its stroke box, and it is offset by its fill box.
  const box = tree.nodeById('box');
  const boxPng = nodePng(box, rect(box.value.absStrokeBoundingBox()), rect(box.value.absBoundingBox()));
  assert.ok(Buffer.compare(boxPng, flat.renderNodePng('box')) === 0);
  // A group's are both its layer box.
  const g = tree.nodeById('g');
  const layer = bbox(g.value.absLayerBoundingBox());
  assert.ok(Buffer.compare(nodePng(g, layer, layer), flat.renderNodePng('g')) === 0);
});

test('renderNode takes only a node reached as one', () => {
  const tree = usvg.Tree.fromStr(SVG);
  const pixmap = new tiny_skia.Pixmap(4, 4);
  const t = tiny_skia.Transform.identity();
  // The root group is not a `usvg::Node`: no node to draw.
  assert.throws(() => resvg.renderNode({ kind: 'group', value: tree.root() }, t, pixmap), { code: 'InvalidArg' });
  assert.throws(() => resvg.renderNode({ kind: 'nope', value: tree.root() }, t, pixmap), { code: 'InvalidArg' });
  // The value's class is checked against the kind.
  const r = tree.nodeById('r');
  assert.throws(() => resvg.renderNode({ kind: 'group', value: r.value }, t, pixmap));
  assert.throws(() => resvg.renderNode({}, t, pixmap));
  assert.equal(resvg.renderNode(r, t, pixmap), true);
});
