// Bounding boxes (`usvg.Group`'s, the root's included), crop, and per-element
// render. 0.9 had these on the flat `Resvg`; the root group has them now.
import assert from 'node:assert/strict';
import { fontsOrSkip, extent, renderNodePng } from './support.mjs';
import binding from '../index.js';
import { parse, renderPng, renderRaw } from '../helpers.mjs';

const { usvg, tiny_skia } = binding;
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds.
const { db, family } = fontsOrSkip(binding, 'bbox + crop');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">
  <g transform="translate(50 20)">
    <rect id="box" x="10" y="10" width="60" height="40" fill="tomato" stroke="black" stroke-width="8"/>
  </g>
</svg>`;
const tree = usvg.Tree.fromStr(svg);
const r = tree.root();
// A `tiny_skia.Rect` or `NonZeroRect`: the sides are methods.
const box = (b) => [b.x(), b.y(), b.width(), b.height()];

// 1. the six variants, and they differ where they should
assert.deepEqual(box(r.boundingBox()), [60, 30, 60, 40], 'geometry only');
assert.deepEqual(box(r.absBoundingBox()), [60, 30, 60, 40]);
assert.deepEqual(box(r.strokeBoundingBox()), [56, 26, 68, 48], 'stroke widens by 4 a side');
assert.deepEqual(box(r.absStrokeBoundingBox()), [56, 26, 68, 48]);
assert.ok(r.layerBoundingBox() instanceof tiny_skia.NonZeroRect, 'a drawn group has a layer box');
assert.deepEqual(box(r.layerBoundingBox()), [56, 26, 68, 48]);
// same numbers resvg-js 2.6.2 reports for getBBox()/innerBBox() on this document
assert.deepEqual(box(r.absLayerBoundingBox()), [56, 26, 68, 48]);

// 2. filters widen the layer box, and only the layer box
const blurred = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <filter id="f"><feGaussianBlur stdDeviation="5"/></filter>
  <g filter="url(#f)"><rect x="50" y="50" width="40" height="40" fill="red"/></g>
</svg>`);
assert.deepEqual(box(blurred.root().absBoundingBox()), [50, 50, 40, 40]);
assert.ok(blurred.root().absLayerBoundingBox().width() > 40, 'blur spills past the geometry');
// filtersBoundingBox reports the ROOT group's own filters, and usvg always
// wraps a filtered element in an inner group -- so at root level it stays null.
assert.equal(blurred.root().filtersBoundingBox(), null, 'root group carries no filter itself');
assert.equal(r.filtersBoundingBox(), null);

// 3. cheap predicates, straight from `impl Tree` / `impl Group`
assert.equal(tree.hasTextNodes(), false);
assert.equal(r.hasChildren(), true);
const texty = parse('<svg xmlns="http://www.w3.org/2000/svg" width="50" height="20"><text x="2" y="15">hi</text></svg>',
  { fonts: db, fontFamily: family }).tree;
assert.equal(texty.hasTextNodes(), true);

// 4. crop trims the viewport, and sizing then applies to the crop: a
//    `NonZeroRect`, or the plain box `nodeExtent` returns
const bb = r.absLayerBoundingBox();
const cropped = renderRaw(tree, { crop: bb });
assert.deepEqual([cropped.width, cropped.height], [68, 48], '300x200 trimmed to its content');
const scaled = renderRaw(tree, { crop: bb, width: 136 });
assert.deepEqual([scaled.width, scaled.height], [136, 96], 'width sizes the crop, not the viewport');
assert.deepEqual([...cropped.data.subarray(0, 4)], [0, 0, 0, 255], 'top-left is the stroke');
assert.ok(renderRaw(tree, { crop: extent({ kind: 'group', value: r }) }).data.equals(cropped.data), 'a plain box');
assert.throws(() => renderPng(tree, { crop: { x: 0, y: 0, width: 0, height: 10 } }), /empty crop/);

// 5. per-element extent includes the stroke, unlike usvg's own node bbox
const ext = extent(tree.nodeById('box'));
assert.deepEqual([ext.x, ext.y, ext.width, ext.height], [56, 26, 68, 48]);
assert.equal(tree.nodeById('nope'), null);

// 6. one node rendered alone through resvg.renderNode is pixel-identical to
//    cropping the document to that element
const decode = (png) => {
  const p = tiny_skia.Pixmap.decodePng(png);
  return { width: p.width(), data: Buffer.from(p.takeDemultiplied()) };
};
const node = decode(renderNodePng(binding, tree.nodeById('box')));
assert.deepEqual([...node.data], [...cropped.data], 'renderNode ≡ crop to the node');
assert.deepEqual([decode(renderNodePng(binding, tree.nodeById('box'), { scale: 2 })).width], [136]);
