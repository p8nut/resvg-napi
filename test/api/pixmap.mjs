// Reading a rendered `tiny_skia.Pixmap` back: one pixel (`pixel`), and a crop
// (`cloneRect` with a `tiny_skia.IntRect`). Neither class is listed in the
// generator config: they are classes because `Pixmap`'s members return and
// take them.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import binding from '../../index.js';

const { usvg, tiny_skia, resvg } = binding;

/** A 4x2 pixmap: the left half opaque red, the right half half-transparent blue. */
function halves() {
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2">
    <rect width="2" height="2" fill="#f00"/>
    <rect x="2" width="2" height="2" fill="#00f" fill-opacity="0.5"/>
  </svg>`);
  const pixmap = new tiny_skia.Pixmap(4, 2);
  resvg.render(tree, tiny_skia.Transform.identity(), pixmap);
  return pixmap;
}

/** A colour's channels as `[r, g, b, a]`. */
const rgba = (c) => [c.red(), c.green(), c.blue(), c.alpha()];
/** A rect's `[x, y, width, height]`. */
const xywh = (r) => [r.x(), r.y(), r.width(), r.height()];

test('pixel reads one premultiplied pixel, null past the last row', () => {
  const pixmap = halves();
  const red = pixmap.pixel(0, 1);
  assert.ok(red instanceof tiny_skia.PremultipliedColorU8);
  assert.deepEqual(rgba(red), [255, 0, 0, 255]);
  assert.equal(red.isOpaque(), true);
  // Premultiplied: half-transparent blue stores half the blue.
  const blue = pixmap.pixel(3, 0);
  assert.deepEqual(rgba(blue), [0, 0, 128, 128]);
  assert.deepEqual(rgba(blue.demultiply()), [0, 0, 255, 128]);
  // As in Rust, the index is `y * width + x`, unchecked per axis: past the
  // last row is null, past the end of a row is the next row.
  assert.equal(pixmap.pixel(0, 2), null);
  assert.deepEqual(rgba(pixmap.pixel(4, 0)), rgba(pixmap.pixel(0, 1)));
});

test('cloneRect crops to an IntRect, clipped to the pixmap, null outside it', () => {
  const pixmap = halves();
  const rect = tiny_skia.IntRect.fromXywh(2, 0, 2, 2);
  assert.ok(rect instanceof tiny_skia.IntRect);
  assert.deepEqual(xywh(rect), [2, 0, 2, 2]);
  const crop = pixmap.cloneRect(rect);
  assert.deepEqual([crop.width(), crop.height()], [2, 2]);
  // The right half of each row: bytes 8..16 of a 16-byte row.
  const data = pixmap.data();
  assert.deepEqual(crop.data(), Buffer.concat([data.subarray(8, 16), data.subarray(24, 32)]));
  // A rect over the edge is clipped to the pixmap, one past it gives none,
  // and an empty rect is no IntRect.
  const clipped = pixmap.cloneRect(tiny_skia.IntRect.fromXywh(3, 0, 2, 2));
  assert.deepEqual([clipped.width(), clipped.height()], [1, 2]);
  assert.equal(pixmap.cloneRect(tiny_skia.IntRect.fromXywh(4, 0, 2, 2)), null);
  assert.equal(tiny_skia.IntRect.fromXywh(0, 0, 0, 2), null);
});

test('IntRect comes from the float rects and sizes, and goes back to a Rect', () => {
  assert.deepEqual(xywh(tiny_skia.Rect.fromXywh(0.4, 0.6, 2.2, 1.1).roundOut()), [0, 0, 3, 2]);
  assert.deepEqual(xywh(tiny_skia.IntSize.fromWh(3, 4).toIntRect(1, 2)), [1, 2, 3, 4]);
  assert.deepEqual(xywh(tiny_skia.IntRect.fromXywh(1, 2, 3, 4).toRect()), [1, 2, 3, 4]);
});
