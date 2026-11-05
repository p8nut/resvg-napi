// `renderPng` / `renderRaw` and their async twins (step 11 of
// docs/migration-0.10.md): JS on the generated API, and the same bytes as the
// flat `Resvg.renderPng` / `renderRaw` for the same document and params. The
// old canvas arithmetic was Rust's f32; the helpers redo it in JS, so these
// compare every sizing path, not a sample PNG.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import binding from '../../index.js';
import helpers, { MAX_PIXELS, renderPng, renderPngAsync, renderRaw, renderRawAsync, parse } from '../../helpers.mjs';
import { testFonts, treeFontsOrSkip } from '../support.mjs';

const { Resvg, FontDatabase, usvg } = binding;

// Text included, so the fonts have to agree: the same faces, loaded alike, in
// the flat `FontDatabase` and in the `fontdb.Database` the helpers take.
const flatFonts = testFonts(FontDatabase);
const { db, family } = treeFontsOrSkip(binding, 'helpers: renderPng byte equality');

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

for (const [doc, svg] of Object.entries(DOCS)) {
  test(`renderPng / renderRaw: the flat Resvg's bytes (${doc})`, async () => {
    const flat = new Resvg(svg, { fontFamily: family }, flatFonts.db);
    const options = { fonts: db, fontFamily: family };
    const { tree } = parse(svg, options);
    for (const params of PARAMS) {
      const want = flat.renderPng(params);
      const wantRaw = flat.renderRaw(params);
      // From source, from a parsed tree, and async: one answer.
      assert.ok(renderPng(svg, { ...options, ...params }).equals(want), `renderPng(svg) ${name(params)}`);
      assert.ok(renderPng(tree, params).equals(want), `renderPng(tree) ${name(params)}`);
      assert.ok((await renderPngAsync(svg, { ...options, ...params })).equals(want), `renderPngAsync ${name(params)}`);
      assert.ok((await renderPngAsync(tree, params)).equals(want), `renderPngAsync(tree) ${name(params)}`);
      for (const raw of [renderRaw(tree, params), await renderRawAsync(svg, { ...options, ...params })]) {
        assert.deepEqual([raw.width, raw.height], [wantRaw.width, wantRaw.height], `renderRaw size ${name(params)}`);
        assert.ok(raw.data.equals(wantRaw.data), `renderRaw pixels ${name(params)}`);
      }
    }
    // A tree the caller passes is the caller's: still usable.
    assert.equal(tree.disposed, false);
  });
}

// The old suite's sweep (test/render.mjs): every requested dimension comes back
// exactly, and as the flat render sized it.
test('every requested width and height, as the flat render sized it', () => {
  for (const [w, h] of [[100, 50], [100, 33], [37, 91], [1, 1]]) {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="${w}" height="${h}" fill="red"/></svg>`;
    const flat = new Resvg(svg);
    const { tree } = parse(svg, { fonts: [] });
    const wrong = [];
    for (let n = 1; n <= 300; n++) {
      for (const key of ['width', 'height']) {
        const a = renderRaw(tree, { [key]: n });
        const b = flat.renderRaw({ [key]: n });
        if (a[key] !== n || a.width !== b.width || a.height !== b.height) wrong.push(`${key}:${n}`);
      }
    }
    assert.deepEqual(wrong, [], `${w}x${h}`);
  }
});

test('maxPixels: refused before allocating, with the flat messages', async () => {
  const svg = DOCS.shapes;
  const flat = new Resvg(svg);
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
    assert.throws(() => flat.renderPng(params), message, `flat ${name(params)}`);
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
