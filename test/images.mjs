// Images: resolving them through the helpers, and reading them back out of
// the tree. What the guard reads and refuses (fences, caps, FIFOs, SVGZ,
// oversized rasters) is test/api/guard.mjs.
import assert from 'node:assert/strict';
import binding from '../index.js';
import { parse, renderPng, renderRaw } from '../helpers.mjs';

const { usvg } = binding;

// a 20x20 red PNG, produced by our own renderer
const red = renderPng('<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="#ff0000"/></svg>', { fonts: [] });

const doc = (href) => `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">
  <image xlink:href="${href}" x="0" y="0" width="20" height="20"/>
</svg>`;

const pixel = (tree) => [...renderRaw(tree).data.subarray(0, 4)];

// 1. Two passes, as the 0.9 `resolveImage` did them: an unresolvable href is
//    reported, not fatal; parsing again with the bytes draws it. `images`
//    takes every href at once, and one the document does not use is no error.
{
  const first = parse(doc('company-logo'), { fonts: [] });
  assert.deepEqual(first.pendingImages, ['company-logo']);
  assert.deepEqual(pixel(first.tree), [0, 0, 0, 0], 'nothing drawn yet');
  const second = parse(doc('company-logo'), { fonts: [], images: { 'company-logo': red, unused: red } });
  assert.deepEqual(second.pendingImages, []);
  assert.deepEqual(pixel(second.tree), [255, 0, 0, 255], 'red image drawn');
}

// --- reading an embedded image back out ------------------------------------
// The bytes a document carried, handed over untouched. usvg says a raster
// payload "should be decoded by the caller", and these are the caller's.
//
// The union variant carrying them is a read-only class rather than an
// `#[napi(object)]`: neither `Buffer` nor `Uint8Array` is `Clone`, which napi
// requires of every field of an object, and a getter has no such constraint.
{
  const b64 = red.toString('base64');
  const parsed = usvg.Tree.fromStr(doc(`data:image/png;base64,${b64}`));

  // usvg wraps the element in a group, so the image is the group's child --
  // the same shape as any other node with an id.
  const [group] = parsed.root().children();
  assert.equal(group.kind, 'group', 'the wrapper is a group, not an image');
  const [node] = group.value.children();
  assert.equal(node.kind, 'image', 'the child is the image itself');
  const img = node.value;
  assert.ok(img instanceof usvg.Image);

  const kind = img.kind();
  assert.equal(kind.type, 'png', 'discriminant, lowercase despite usvg spelling it PNG');
  assert.ok(Buffer.isBuffer(kind.bytes));
  assert.equal(Buffer.compare(kind.bytes, red), 0, 'byte-for-byte what went in');

  // and it is a real PNG on the way out, not a re-encode
  assert.equal(kind.bytes.subarray(0, 4).toString('hex'), '89504e47');

  // the rest of the node reads back too
  assert.equal(img.renderingMode(), 'optimizeQuality');

  // `size` is the encoded image's own, not the element's width/height
  const wide = renderPng('<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>', { fonts: [], width: 40 });
  const shown = usvg.Tree.fromStr(doc(`data:image/png;base64,${wide.toString('base64')}`));
  const size = shown.root().children()[0].value.children()[0].value.size();
  assert.deepEqual([size.width(), size.height()], [40, 40]);
  assert.deepEqual(
    [img.boundingBox().width(), img.boundingBox().height()],
    [20, 20],
    'the box is the image, in its own pixels',
  );
}

// 2. An embedded SVG is a kind of its own. Its tree (`ImageKind::SVG`, a tree
//    held by value inside a node) has no form yet (#191): only the kind reads
//    back.
{
  const svgHref = `data:image/svg+xml;utf8,${encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>')}`;
  const outer = usvg.Tree.fromStr(doc(svgHref));
  const image = outer.root().children()[0].value.children()[0];
  assert.equal(image.kind, 'image');
  assert.equal(image.value.kind().type, 'svg');
}
