// Rendering through the namespaced surface, the way Rust does it:
//
//   usvg.Tree.fromStr(svg, { fontdb }) -> new tiny_skia.Pixmap(w, h)
//   -> resvg.render(tree, transform, pixmap) -> pixmap.encodePng()
//
// and the same PNG, byte for byte, as the flat `Resvg.renderPng` at the same
// scale with the same fonts: the two surfaces drive the same crates.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';

const { usvg, fontdb, tiny_skia, resvg } = binding;

// A font the repository carries, so both surfaces load the same bytes
// natively and under WASI alike.
const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));
const FAMILY = 'IBM Plex Sans';

const SVGS = {
  shapes: `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="40" viewBox="0 0 32 20">
    <defs><linearGradient id="g"><stop offset="0" stop-color="#e33"/><stop offset="1" stop-color="#33e"/></linearGradient></defs>
    <rect x="1" y="1" width="30" height="18" rx="4" fill="url(#g)" stroke="#111" stroke-width="0.5"/>
    <circle cx="16" cy="10" r="6" fill="#fff" fill-opacity="0.6"/>
  </svg>`,
  text: `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40">
    <text x="4" y="28" font-family="${FAMILY}" font-size="22" fill="#222">Hello</text>
  </svg>`,
  effects: `<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60">
    <defs>
      <filter id="b"><feGaussianBlur stdDeviation="2"/></filter>
      <clipPath id="c"><circle cx="40" cy="30" r="22"/></clipPath>
      <mask id="m"><rect width="80" height="60" fill="white"/><rect x="30" width="20" height="60" fill="black"/></mask>
    </defs>
    <g clip-path="url(#c)" mask="url(#m)"><rect width="80" height="60" fill="teal" filter="url(#b)"/></g>
  </svg>`,
  pattern: `<svg xmlns="http://www.w3.org/2000/svg" width="50" height="50">
    <defs><pattern id="p" width="10" height="10" patternUnits="userSpaceOnUse" patternTransform="rotate(30)">
      <rect width="5" height="5" fill="orange"/></pattern></defs>
    <rect width="50" height="50" fill="url(#p)" transform="translate(25 25) scale(0.8) translate(-25 -25)"/>
  </svg>`,
};

/** The flat surface's PNG. */
function flatPng(svg, scale) {
  const db = new binding.FontDatabase();
  db.loadFontData(FONT);
  return new binding.Resvg(svg, { fontFamily: FAMILY }, db).renderPng({ scale });
}

/** The same through usvg, tiny-skia and resvg, as their Rust APIs read. */
function namespacedPng(svg, scale) {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const tree = usvg.Tree.fromStr(svg, { fontdb: db, fontFamily: FAMILY });
  const size = tree.size();
  const pixmap = new tiny_skia.Pixmap(Math.ceil(size.width() * scale), Math.ceil(size.height() * scale));
  resvg.render(tree, tiny_skia.Transform.fromScale(scale, scale), pixmap);
  return pixmap.encodePng();
}

for (const [name, svg] of Object.entries(SVGS)) {
  test(`${name}: the namespaced render is the flat one, byte for byte`, () => {
    for (const scale of [1, 2]) {
      const a = flatPng(svg, scale);
      const b = namespacedPng(svg, scale);
      assert.ok(b.length > 100, `${name} @${scale}x is a PNG`);
      assert.ok(Buffer.compare(a, b) === 0, `${name} @${scale}x: same bytes`);
    }
  });
}

test('the text test draws something: the font reached the render', () => {
  const png = namespacedPng(SVGS.text, 1);
  const pixmap = tiny_skia.Pixmap.decodePng(png);
  assert.ok(pixmap.data().some((v) => v !== 0));
});

test('the Rust render loop, step by step', () => {
  const tree = usvg.Tree.fromData(Buffer.from(SVGS.shapes));
  const size = tree.size();
  assert.ok(size instanceof tiny_skia.Size);
  assert.deepEqual([size.width(), size.height()], [64, 40]);
  const pixmap = new tiny_skia.Pixmap(64, 40);
  assert.equal(pixmap.data().length, 64 * 40 * 4);
  assert.ok(pixmap.data().every((v) => v === 0), 'a new pixmap is transparent');
  pixmap.fill(tiny_skia.Color.fromRgba8(255, 255, 255, 255));
  assert.ok(pixmap.data().every((v) => v === 255), 'fill() writes every pixel');
  resvg.render(tree, tiny_skia.Transform.identity(), pixmap);
  // The circle's centre: white over the gradient, at 60 % opacity.
  const i = (20 * 64 + 32) * 4;
  assert.ok(pixmap.data()[i + 3] === 255);
  const png = pixmap.encodePng();
  assert.deepEqual([...png.subarray(1, 4)], [0x50, 0x4e, 0x47], 'PNG signature');
  // `take` consumes the pixmap, as in Rust: the bytes leave with it.
  const bytes = pixmap.take();
  assert.equal(bytes.length, 64 * 40 * 4);
  assert.throws(() => pixmap.width(), { code: 'Consumed' });
});

test('a transform positions the drawing, as in Rust', () => {
  const tree = usvg.Tree.fromStr('<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#f00"/></svg>');
  const pixmap = new tiny_skia.Pixmap(8, 8);
  resvg.render(tree, tiny_skia.Transform.fromTranslate(4, 4), pixmap);
  const px = (x, y) => [...pixmap.data().subarray((y * 8 + x) * 4, (y * 8 + x) * 4 + 4)];
  assert.deepEqual(px(1, 1), [0, 0, 0, 0]);
  assert.deepEqual(px(6, 6), [255, 0, 0, 255]);
});

test('a tree is shared, not copied: render it into two pixmaps', () => {
  const tree = usvg.Tree.fromStr(SVGS.shapes);
  const a = new tiny_skia.Pixmap(64, 40);
  const b = new tiny_skia.Pixmap(64, 40);
  resvg.render(tree, tiny_skia.Transform.identity(), a);
  resvg.render(tree, tiny_skia.Transform.identity(), b);
  assert.ok(Buffer.compare(a.data(), b.data()) === 0);
});

test('render takes only its classes', () => {
  const tree = usvg.Tree.fromStr(SVGS.shapes);
  const pixmap = new tiny_skia.Pixmap(4, 4);
  const t = tiny_skia.Transform.identity();
  assert.throws(() => resvg.render({}, t, pixmap));
  assert.throws(() => resvg.render(tree, { sx: 1, kx: 0, ky: 0, sy: 1, tx: 0, ty: 0 }, pixmap));
  assert.throws(() => resvg.render(tree, t, tiny_skia.IntSize.fromWh(4, 4)));
});
