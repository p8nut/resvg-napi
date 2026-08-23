// BBox (generated from `impl Group` / `impl Tree`), crop, and per-element render.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg } = createRequire(import.meta.url)('./index.js');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">
  <g transform="translate(50 20)">
    <rect id="box" x="10" y="10" width="60" height="40" fill="tomato" stroke="black" stroke-width="8"/>
  </g>
</svg>`;
const r = new Resvg(svg);
const box = (b) => [b.x, b.y, b.width, b.height];

// 1. the six generated variants, and they differ where they should
assert.deepEqual(box(r.boundingBox()), [60, 30, 60, 40], 'geometry only');
assert.deepEqual(box(r.absBoundingBox()), [60, 30, 60, 40]);
assert.deepEqual(box(r.strokeBoundingBox()), [56, 26, 68, 48], 'stroke widens by 4 a side');
assert.deepEqual(box(r.absStrokeBoundingBox()), [56, 26, 68, 48]);
assert.deepEqual(box(r.layerBoundingBox()), [56, 26, 68, 48]);
// same numbers resvg-js 2.6.2 reports for getBBox()/innerBBox() on this document
assert.deepEqual(box(r.absLayerBoundingBox()), [56, 26, 68, 48]);

// 2. filters widen the layer box, and only the layer box
const blurred = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <filter id="f"><feGaussianBlur stdDeviation="5"/></filter>
  <g filter="url(#f)"><rect x="50" y="50" width="40" height="40" fill="red"/></g>
</svg>`);
assert.deepEqual(box(blurred.absBoundingBox()), [50, 50, 40, 40]);
assert.ok(blurred.absLayerBoundingBox().width > 40, 'blur spills past the geometry');
// filtersBoundingBox reports the ROOT group's own filters, and usvg always
// wraps a filtered element in an inner group -- so at root level it stays null.
assert.equal(blurred.filtersBoundingBox(), null, 'root group carries no filter itself');
assert.equal(r.filtersBoundingBox(), null);

// 3. cheap predicates, straight from `impl Tree` / `impl Group`
assert.equal(r.hasTextNodes(), false);
assert.equal(r.hasDefsNodes(), false);
assert.equal(r.hasChildren(), true);
assert.equal(blurred.hasDefsNodes(), true, 'the filter lives in defs');
const texty = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="50" height="20"><text x="2" y="15">hi</text></svg>',
  { fontFamily: 'DejaVu Sans' });
assert.equal(texty.hasTextNodes(), true);

// 4. crop trims the viewport, and sizing then applies to the crop
const bb = r.absLayerBoundingBox();
const cropped = r.renderRaw({ crop: bb });
assert.deepEqual([cropped.width, cropped.height], [68, 48], '300x200 trimmed to its content');
const scaled = r.renderRaw({ crop: bb, width: 136 });
assert.deepEqual([scaled.width, scaled.height], [136, 96], 'width sizes the crop, not the viewport');
assert.deepEqual([...cropped.data.subarray(0, 4)], [0, 0, 0, 255], 'top-left is the stroke');
assert.throws(() => r.renderPng({ crop: { x: 0, y: 0, width: 0, height: 10 } }), /empty crop/);

// 5. per-element bbox includes the stroke, unlike usvg's own node bbox
assert.deepEqual(box(r.nodeBbox('box')), [56, 26, 68, 48]);
assert.equal(r.nodeBbox('nope'), null);

// 6. renderNodePng must be pixel-identical to cropping to that element
const decode = (png, w, h) =>
  new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="${w}" height="${h}"><image xlink:href="n" width="${w}" height="${h}"/></svg>`,
    null, null, { n: png }).renderRaw();
const node = decode(r.renderNodePng('box'), 68, 48);
assert.deepEqual([...node.data], [...cropped.data], 'renderNodePng ≡ crop to the node');
assert.deepEqual([decode(r.renderNodePng('box', { scale: 2 }), 136, 96).width], [136]);
assert.ok(r.renderNodePng('box', { background: 'teal' }).length > 0);
assert.throws(() => r.renderNodePng('nope'), /no element with id/);

console.log('ok — bbox + crop + renderNode: all checks passed');
