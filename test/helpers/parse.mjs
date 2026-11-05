// `parse` / `parseAsync` (step 11 of docs/migration-0.10.md): the guard on by
// default, the flat `Resvg`'s `pendingImages` / `pendingFonts`, the SVGZ
// advice of #167, the system fonts scanned once, and `createHelpers(binding)`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import zlib from 'node:zlib';
import binding from '../../index.js';
import helpers, { createHelpers, parse, parseAsync, renderPng, systemFonts, systemFontsAsync } from '../../helpers.mjs';

const { Resvg, usvg, fontdb, tiny_skia, resvg } = binding;

/** A 20x20 PNG of one colour. */
function png(r, g, b) {
  const p = new tiny_skia.Pixmap(20, 20);
  p.fill(tiny_skia.Color.fromRgba8(r, g, b, 255));
  return p.encodePng();
}
const RED = png(255, 0, 0);

const doc = (...hrefs) => `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">
  ${hrefs.map((h) => `<image xlink:href="${h}" x="0" y="0" width="20" height="20"/>`).join('')}
  <text x="2" y="15" font-family="No Such Family, Another Missing One">hi</text>
</svg>`;

/** The first pixel of `tree` rendered at its size. */
function pixel(tree) {
  const pixmap = new tiny_skia.Pixmap(20, 20);
  resvg.render(tree, tiny_skia.Transform.identity(), pixmap);
  return [...pixmap.data().subarray(0, 4)];
}

const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));

test('pendingImages and pendingFonts: what the flat Resvg reports', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'resvg-helpers-'));
  writeFileSync(join(dir, 'disk.png'), RED);
  const outside = mkdtempSync(join(tmpdir(), 'resvg-helpers-out-'));
  writeFileSync(join(outside, 'secret.png'), RED);
  const svg = doc('logo', 'disk.png', 'missing.png', join(outside, 'secret.png'), `../${outside.split(/[\\/]/).pop()}/secret.png`);
  const cases = [
    [{}, {}],
    [{ resourcesDir: dir }, {}],
    [{ resourcesDir: dir }, { logo: RED }],
    [{ resourcesDir: dir, resourcesRoot: tmpdir() }, { logo: RED }],
  ];
  for (const [options, images] of cases) {
    const flat = new Resvg(svg, options, null, images);
    const want = [flat.pendingImages(), flat.pendingFonts()];
    const sync = parse(svg, { ...options, images });
    const async = await parseAsync(Buffer.from(svg), { ...options, images });
    for (const got of [sync, async]) {
      assert.deepEqual([got.pendingImages, got.pendingFonts], want, JSON.stringify(options));
      assert.ok(got.tree instanceof usvg.Tree);
    }
    assert.ok(renderPng(sync.tree).equals(flat.renderPng()), 'and the same pixels');
  }
});

test('the guard is the default: a file outside resourcesDir is not read', () => {
  const outside = mkdtempSync(join(tmpdir(), 'resvg-helpers-out-'));
  const secret = join(outside, 'secret.png');
  writeFileSync(secret, RED);
  const inside = mkdtempSync(join(tmpdir(), 'resvg-helpers-in-'));
  // The raw API reads it (test/api/guard.mjs); the helpers do not.
  assert.deepEqual(pixel(usvg.Tree.fromStr(doc(secret))), [255, 0, 0, 255], 'raw: read');
  for (const options of [{}, { resourcesDir: inside }, { fonts: [] }]) {
    const { tree, pendingImages } = parse(doc(secret), options);
    assert.deepEqual(pixel(tree), [0, 0, 0, 0], 'helpers: not read');
    assert.deepEqual(pendingImages, [secret]);
  }
  // Supplied images and resourcesDir still work.
  assert.deepEqual(pixel(parse(doc('x'), { images: { x: RED } }).tree), [255, 0, 0, 255]);
  writeFileSync(join(inside, 'in.png'), RED);
  assert.deepEqual(pixel(parse(doc('in.png'), { resourcesDir: inside }).tree), [255, 0, 0, 255]);
});

test('a resolver of your own is used, not disposed, and reports through parse', () => {
  const { guard } = binding;
  const images = new guard.ImageHrefResolver({ images: { a: RED } });
  const fonts = new guard.FontResolver();
  const out = parse(doc('a', 'b'), { imageHrefResolver: images, fontResolver: fonts });
  assert.deepEqual(out.pendingImages, ['b']);
  assert.deepEqual(images.misses(), ['b']);
  assert.deepEqual(out.pendingFonts, fonts.misses());
  assert.equal(images.disposed, false);
});

test('SVGZ: refused before parsing, with advice (#167)', async () => {
  const red = '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>';
  const refused = {
    code: 'SvgzFeatureNotEnabled',
    message: 'SVGZ is not supported: inflate it first, e.g. zlib.gunzipSync(data, { maxOutputLength })',
  };
  for (const data of [zlib.gzipSync(red), Buffer.from([0x1f, 0x8b, 1, 2]), new Uint8Array([0x1f, 0x8b])]) {
    assert.throws(() => parse(data), refused);
    assert.throws(() => renderPng(data), refused);
    await assert.rejects(parseAsync(data), refused);
    await assert.rejects(helpers.renderPngAsync(data), refused);
  }
  // What the message advises works.
  assert.equal(parse(zlib.gunzipSync(zlib.gzipSync(red), { maxOutputLength: 1 << 20 })).tree.size().width(), 10);
  // Other parse errors are the binding's, unchanged.
  assert.throws(() => parse('<svg'), { code: 'ParsingFailed' });
  assert.throws(() => parse(Buffer.from([0xff, 0xfe, 0x3c])), { code: 'NotAnUtf8Str' });
  await assert.rejects(parseAsync('<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/>'), { code: 'InvalidSize' });
  assert.throws(() => parse(42), TypeError);
});

test('fonts: a database, files and buffers, or the system fonts', () => {
  const text = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><text x="2" y="15" font-family="IBM Plex Sans">hi</text></svg>';
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  assert.deepEqual(parse(text, { fonts: db }).pendingFonts, []);
  assert.deepEqual(parse(text, { fontdb: db }).pendingFonts, [], 'usvg\'s name for it');
  assert.deepEqual(parse(text, { fonts: [FONT] }).pendingFonts, []);
  const file = new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url).pathname;
  if (existsSync(file)) assert.deepEqual(parse(text, { fonts: [file] }).pendingFonts, []);
  assert.deepEqual(parse(text, { fonts: [] }).pendingFonts, ['IBM Plex Sans'], 'an empty list is no fonts');
  assert.ok(renderPng(text, { fonts: db }).equals(renderPng(text, { fonts: [FONT] })));
  assert.throws(() => parse(text, { fonts: 'x' }), TypeError);
  // No `fonts`: the system's, one database shared.
  assert.ok(systemFonts() instanceof fontdb.Database);
  assert.equal(systemFonts(), systemFonts());
});

/**
 * The binding, with `fontdb.Database` counted: how many are made and how many
 * system scans run. The constructor returns the real instance, so the binding
 * still recognises it.
 */
function counted() {
  const count = { made: 0, scans: 0 };
  const Real = binding.fontdb.Database;
  function Database() {
    const db = new Real();
    count.made++;
    const sync = db.loadSystemFonts.bind(db);
    const async = db.loadSystemFontsAsync.bind(db);
    db.loadSystemFonts = () => { count.scans++; return sync(); };
    db.loadSystemFontsAsync = (s) => { count.scans++; return async(s); };
    return db;
  }
  Object.defineProperty(Database, Symbol.hasInstance, { value: (v) => v instanceof Real });
  return { count, binding: { ...binding, fontdb: { ...binding.fontdb, Database } } };
}

test('system fonts: scanned once per binding, sync or async', async () => {
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>';
  {
    const { count, binding: b } = counted();
    const h = createHelpers(b);
    await Promise.all([h.parseAsync(svg), h.parseAsync(svg), h.renderPngAsync(svg)]);
    h.parse(svg);
    h.renderPng(svg);
    await h.parseAsync(svg);
    assert.equal(count.scans, 1, 'one scan');
    assert.equal(h.systemFonts(), await h.systemFontsAsync(), 'one database');
    // A database of one's own is not the system's, and scans nothing.
    h.parse(svg, { fonts: [] });
    assert.equal(count.scans, 1);
  }
  {
    const { count, binding: b } = counted();
    const h = createHelpers(b);
    h.parse(svg);
    await h.parseAsync(svg);
    assert.equal(count.scans, 1, 'sync first, then async: one scan');
  }
  // The default set is this binding's, and memoised.
  assert.equal(createHelpers(binding), helpers);
  assert.equal(await systemFontsAsync(), systemFonts());
  // An aborted wait rejects; the scan it shared goes on.
  const ac = new AbortController();
  ac.abort();
  await assert.rejects(createHelpers(counted().binding).systemFontsAsync(ac.signal));
});

test('createHelpers: an explicit binding, and a refusal without one', () => {
  assert.throws(() => createHelpers(), TypeError);
  assert.throws(() => createHelpers({ usvg }), TypeError);
  // A binding loaded by hand, the way a page passes the wasm module: here the
  // WASI build when it was built (npm run demo:wasm), else index.js again.
  const require = createRequire(import.meta.url);
  const wasm = new URL('../../resvg-napi.wasm32-wasi.wasm', import.meta.url);
  const other = existsSync(wasm) ? require('../../resvg-napi.wasi.cjs') : require('../../index.js');
  const h = createHelpers(other);
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"><rect width="8" height="4" fill="#3b82f6"/></svg>';
  const raw = h.renderRaw(svg, { fonts: [], scale: 2, background: 'white' });
  assert.deepEqual([raw.width, raw.height], [16, 8]);
  assert.deepEqual([...raw.data.subarray(0, 4)], [0x3b, 0x82, 0xf6, 0xff]);
  assert.ok(h.renderPng(svg, { fonts: [] }).equals(renderPng(svg, { fonts: [] })), `${other.__napiBindingTarget} and ${binding.__napiBindingTarget} agree`);
  console.log(`  createHelpers over ${other.__napiBindingTarget}`);
});
