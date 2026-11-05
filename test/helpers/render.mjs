// `renderPng` / `renderRaw` and their async twins (step 11 of
// docs/migration-0.10.md): JS on the generated API. The canvas arithmetic was
// Rust's f32 in 0.9 (`Resvg.renderPng` / `renderRaw`); the helpers redo it in
// JS, and until 0.10 removed the flat `Resvg` these compared their bytes over
// every sizing path. The sizes 0.9 computed are frozen below; the pixels are
// the raw API's (resvg.render on a tiny_skia.Pixmap), and every path of the
// helpers (source, tree, async, PNG or raw) gives the same ones.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import binding from '../../index.js';
import helpers, { MAX_PIXELS, renderPng, renderPngAsync, renderRaw, renderRawAsync, parse } from '../../helpers.mjs';
import { fontsOrSkip } from '../support.mjs';

const { usvg, resvg, tiny_skia } = binding;

const { db, family } = fontsOrSkip(binding, 'helpers: renderPng');

const DOCS = {
  shapes: `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
    <rect width="100" height="50" fill="#3b82f6"/>
    <circle cx="70" cy="25" r="18" fill="rgba(255, 200, 0, .6)" stroke="black" stroke-width="3"/>
    <text x="10" y="30" font-size="16" fill="white">hi</text>
  </svg>`,
  // Odd sizes and a viewBox, so the ratios are not round.
  odd: `<svg xmlns="http://www.w3.org/2000/svg" width="37.3" height="91.7" viewBox="0 0 12 29">
    <path d="M1 1 L11 3 L6 27 Z" fill="tomato" stroke="navy" stroke-width="0.7"/>
    <g opacity="0.5" transform="rotate(12 6 14)"><rect x="2" y="5" width="7" height="9" fill="teal"/></g>
  </svg>`,
  filtered: `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48">
    <filter id="b"><feGaussianBlur stdDeviation="3"/></filter>
    <g filter="url(#b)"><circle cx="32" cy="24" r="14" fill="purple"/></g>
  </svg>`,
};

const PARAMS = [
  undefined,
  {},
  { scale: 2 },
  { scale: 0.37 },
  { scale: 1.5, background: 'white' },
  { width: 120 },
  { width: 120.4 },
  { width: 1 },
  { width: 333 },
  { height: 77 },
  { height: 7, width: 9 }, // width wins
  { background: '#eee' },
  { background: 'teal' },
  { background: 'rgba(255, 0, 0, .5)' },
  { background: 'hsl(120, 50%, 50%)' },
  { background: 'transparent' },
  { crop: { x: 10, y: 5, width: 50, height: 30 } },
  { crop: { x: 10.25, y: 5.5, width: 50.3, height: 30.7 }, width: 200 },
  { crop: { x: -5, y: -5, width: 20, height: 20 }, scale: 3, background: 'black' },
  { width: 20, maxPixels: 1000 },
];

const name = (p) => JSON.stringify(p) ?? 'none';

// The canvas 0.9's Rust `canvas()` sized for each of `PARAMS`, in order: the
// numbers the helpers matched byte for byte until the flat `Resvg` left.
const SIZES = {
  shapes: '100x50 100x50 200x100 37x19 150x75 120x60 120x60 1x1 333x167 154x77 9x5 100x50 100x50 100x50 100x50 100x50 50x30 200x123 60x60 20x10',
  odd: '38x92 38x92 75x184 14x34 56x138 120x296 120x296 1x3 333x819 32x77 9x23 38x92 38x92 38x92 38x92 38x92 50x30 200x123 60x60 20x50',
  filtered: '64x48 64x48 128x96 24x18 96x72 120x90 120x90 1x1 333x250 103x77 9x7 64x48 64x48 64x48 64x48 64x48 50x30 200x123 60x60 20x15',
};

for (const [doc, svg] of Object.entries(DOCS)) {
  test(`renderPng / renderRaw: 0.9's sizes, one answer from every path (${doc})`, async () => {
    const options = { fonts: db, fontFamily: family };
    const { tree } = parse(svg, options);
    const sizes = SIZES[doc].split(' ');
    for (const [i, params] of PARAMS.entries()) {
      const want = renderPng(tree, params);
      const wantRaw = renderRaw(tree, params);
      assert.equal(`${wantRaw.width}x${wantRaw.height}`, sizes[i], `size ${name(params)}`);
      assert.deepEqual([want.readUInt32BE(16), want.readUInt32BE(20)], [wantRaw.width, wantRaw.height], `IHDR ${name(params)}`);
      // From source, from a parsed tree, and async: one answer.
      assert.ok(renderPng(svg, { ...options, ...params }).equals(want), `renderPng(svg) ${name(params)}`);
      assert.ok((await renderPngAsync(svg, { ...options, ...params })).equals(want), `renderPngAsync ${name(params)}`);
      assert.ok((await renderPngAsync(tree, params)).equals(want), `renderPngAsync(tree) ${name(params)}`);
      const raw = await renderRawAsync(svg, { ...options, ...params });
      assert.deepEqual([raw.width, raw.height], [wantRaw.width, wantRaw.height], `renderRawAsync size ${name(params)}`);
      assert.ok(raw.data.equals(wantRaw.data), `renderRawAsync pixels ${name(params)}`);
    }
    // A tree the caller passes is the caller's: still usable.
    assert.equal(tree.disposed, false);
  });
}

test('the pixels are the raw API\'s: resvg.render on a tiny_skia.Pixmap', () => {
  const { tree } = parse(DOCS.odd, { fonts: [] });
  const { width, height, data } = renderRaw(tree, { width: 120 });
  const pixmap = new tiny_skia.Pixmap(width, height);
  const scale = Math.fround(120 / tree.size().width());
  resvg.render(tree, tiny_skia.Transform.fromScale(scale, scale), pixmap);
  assert.ok(Buffer.from(pixmap.takeDemultiplied()).equals(data));
});

// The old suite's sweep (test/render.mjs): every requested dimension comes back
// exactly, and the other side as 0.9 sized it (the sizes' digest, frozen).
test('every requested width and height, as 0.9 sized them', () => {
  const all = [];
  const wrong = [];
  for (const [w, h] of [[100, 50], [100, 33], [37, 91], [1, 1]]) {
    const { tree } = parse(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"/>`, { fonts: [] });
    for (let n = 1; n <= 300; n++) {
      for (const key of ['width', 'height']) {
        const r = renderRaw(tree, { [key]: n });
        if (r[key] !== n) wrong.push(`${w}x${h} ${key}:${n}`);
        all.push(`${r.width}x${r.height}`);
      }
    }
  }
  assert.deepEqual(wrong, []);
  assert.equal(createHash('sha256').update(all.join(',')).digest('hex'),
    '99524f31c15dd875525a6ed1175543b8fc523cc300d4373c39a2c94e2b07470a');
});

test('maxPixels: refused before allocating, with 0.9\'s messages', async () => {
  const svg = DOCS.shapes;
  const { tree } = parse(svg, { fonts: [] });
  const cases = [
    [{ scale: 2000 }, /200000x100000 exceeds maxPixels \(67108864\)$/],
    [{ width: 20, maxPixels: 199 }, /20x10 exceeds maxPixels \(199\)$/],
    [{ maxPixels: NaN }, /invalid maxPixels: NaN$/],
    [{ maxPixels: 0 }, /invalid maxPixels: 0$/],
    [{ width: -1 }, /invalid width: -1$/],
    [{ width: 0 }, /invalid width: 0$/],
    [{ width: 2 ** 32 + 10 }, /invalid width: 4294967306$/],
    [{ width: NaN }, /invalid width: NaN$/],
    [{ height: -5 }, /invalid height: -5$/],
    [{ scale: -1 }, /invalid scale: -1$/],
    [{ scale: NaN }, /invalid scale: NaN$/],
    [{ crop: { x: NaN, y: 0, width: 10, height: 10 } }, /invalid crop: NaN,0 10x10$/],
    [{ crop: { x: 0, y: 0, width: 0, height: 10 } }, /empty crop: 0x10$/],
    [{ background: 'notacolour' }, /invalid background: notacolour$/],
  ];
  for (const [params, message] of cases) {
    assert.throws(() => renderPng(tree, params), message, `renderPng ${name(params)}`);
    assert.throws(() => renderRaw(tree, params), message, `renderRaw ${name(params)}`);
    await assert.rejects(renderPngAsync(tree, params), message, `renderPngAsync ${name(params)}`);
  }
  // A document that declares a huge canvas: an error, not an abort.
  const huge = '<svg xmlns="http://www.w3.org/2000/svg" width="200000" height="100000"/>';
  assert.throws(() => renderPng(huge, { fonts: [] }), /exceeds maxPixels/);
  assert.equal(MAX_PIXELS, 2 ** 26, 'the default, ADR 0002 Q25');
});

test('the async twins: abortable, and the tree stays usable', async () => {
  const { tree } = parse(DOCS.filtered, { fonts: [] });
  const ac = new AbortController();
  ac.abort();
  await assert.rejects(renderPngAsync(tree, { signal: ac.signal }));
  await assert.rejects(renderPngAsync(DOCS.filtered, { signal: ac.signal, fonts: [] }));
  assert.ok((await renderPngAsync(tree)).equals(renderPng(tree)));
  assert.equal(tree.disposed, false);
});

test('the default export is the named exports', () => {
  for (const k of ['parse', 'parseAsync', 'renderPng', 'renderPngAsync', 'renderRaw',
    'renderRawAsync', 'renderNodePng', 'renderNodePngAsync', 'measureText', 'fitTextWidths',
    'systemFonts', 'systemFontsAsync']) {
    assert.equal(typeof helpers[k], 'function', k);
  }
  assert.equal(helpers.renderPng, renderPng);
  assert.equal(Object.isFrozen(helpers), true);
  assert.ok(usvg.Tree);
});
