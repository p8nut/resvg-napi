// `dispose()` on every class of the namespaced surface (ADR 0001): after it,
// every member throws `Disposed`, and `[Symbol.dispose]` is the same function
// where the runtime has the symbol, so `using` calls it. A class holding a
// shared value (a tree, a node of one) drops its own reference only.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { inspect } from 'node:util';
import binding from '../../index.js';

const { usvg, fontdb, tiny_skia, svgtypes, resvg, guard } = binding;
const NAMESPACES = { usvg, fontdb, tiny_skia, svgtypes, resvg, guard };

const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));
const PNG = new tiny_skia.Pixmap(1, 1).encodePng().toString('base64');

const SVG = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100">
  <defs>
    <linearGradient id="lg"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
    <radialGradient id="rg"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></radialGradient>
    <pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="2" height="2"/></pattern>
    <clipPath id="c"><rect width="50" height="50"/></clipPath>
    <mask id="m"><rect width="50" height="50" fill="white"/></mask>
    <filter id="f"><feGaussianBlur stdDeviation="1"/><feImage xlink:href="#r"/></filter>
  </defs>
  <g id="g" clip-path="url(#c)" mask="url(#m)" filter="url(#f)">
    <rect id="r" width="10" height="10" fill="url(#lg)" stroke="url(#rg)"/>
    <rect width="10" height="10" fill="url(#p)"/>
  </g>
  <image id="i" width="1" height="1" xlink:href="data:image/png;base64,${PNG}"/>
  <text id="t" x="10" y="50" font-family="IBM Plex Sans" font-size="12">Hi</text>
</svg>`;

/** One instance of every class, by `namespace.Class`: fresh each call. */
function everyClass() {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const tree = usvg.Tree.fromStr(SVG, { fontdb: db });
  const node = (id) => tree.nodeById(id).value;
  const text = node('t');
  // usvg wraps an `<image>` in a group, which takes its id.
  const image = node('i').children()[0].value;
  const [span] = text.layouted();
  const [feImage] = tree.filters().flatMap((f) => f.primitives()).filter((p) => p.kind.type === 'image');
  return {
    'fontdb.Database': db,
    'fontdb.FaceInfo': db.faces()[0],
    'fontdb.ID': db.faces()[0].id,
    'svgtypes.Color': svgtypes.Color.fromStr('red'),
    'tiny_skia.Color': tiny_skia.Color.fromRgba8(1, 2, 3, 4),
    'tiny_skia.IntSize': tiny_skia.IntSize.fromWh(1, 2),
    'tiny_skia.Pixmap': new tiny_skia.Pixmap(2, 2),
    'tiny_skia.Rect': tiny_skia.Rect.fromLtrb(0, 0, 1, 1),
    'tiny_skia.NonZeroRect': tiny_skia.NonZeroRect.fromXywh(0, 0, 1, 1),
    'tiny_skia.Point': tiny_skia.Point.fromXy(1, 2),
    'tiny_skia.Size': tiny_skia.Size.fromWh(1, 2),
    'tiny_skia.Transform': tiny_skia.Transform.identity(),
    'usvg.Tree': tree,
    'usvg.Group': node('g'),
    'usvg.Path': node('r'),
    'usvg.Image': image,
    'usvg.Text': text,
    'usvg.ClipPath': tree.clipPaths()[0],
    'usvg.Mask': tree.masks()[0],
    'usvg.Pattern': tree.patterns()[0],
    'usvg.Filter': tree.filters()[0],
    'usvg.Primitive': tree.filters()[0].primitives()[0],
    'usvg.LinearGradient': tree.linearGradients()[0],
    'usvg.RadialGradient': tree.radialGradients()[0],
    'usvg.FilterImage': feImage.kind.value,
    'usvg.Span': span,
    'usvg.PositionedGlyph': span.positionedGlyphs[0],
    'usvg.ImageKindPng': image.kind(),
    'guard.ImageHrefResolver': new guard.ImageHrefResolver(),
    'guard.FontResolver': new guard.FontResolver(),
  };
}

/** Every class the namespaces export, by `namespace.Class`. */
function exported() {
  const out = {};
  for (const [ns, members] of Object.entries(NAMESPACES)) {
    for (const [name, value] of Object.entries(members)) {
      // A class has members on its prototype; a free function has none.
      if (typeof value === 'function' && Object.getOwnPropertyNames(value.prototype ?? {}).length > 1) {
        out[`${ns}.${name}`] = value;
      }
    }
  }
  return out;
}

/** The instance members of a class: its getters and its methods. */
function members(proto) {
  const getters = [];
  const methods = [];
  for (const [key, d] of Object.entries(Object.getOwnPropertyDescriptors(proto))) {
    if (['constructor', 'dispose', 'disposed'].includes(key)) continue;
    if (d.get) getters.push(key);
    else if (typeof d.value === 'function') methods.push(key);
  }
  return { getters, methods };
}

const disposedError = (name) => ({ code: 'Disposed', message: `this ${name} was disposed` });

test('every class has dispose(), and the test reaches every one', () => {
  const classes = exported();
  for (const [key, cls] of Object.entries(classes)) {
    assert.equal(typeof cls.prototype.dispose, 'function', key);
    assert.ok(Object.getOwnPropertyDescriptor(cls.prototype, 'disposed')?.get, key);
  }
  // ImageKindJpeg, Gif and Webp are the same generated shape as Png.
  const unreached = ['usvg.ImageKindGif', 'usvg.ImageKindJpeg', 'usvg.ImageKindWebp'];
  assert.deepEqual(
    Object.keys(classes).filter((k) => !unreached.includes(k)).sort(),
    Object.keys(everyClass()).sort(),
  );
  for (const [key, it] of Object.entries(everyClass())) {
    assert.ok(it instanceof classes[key], key);
  }
});

test('[Symbol.dispose] is dispose, where the runtime has the symbol', () => {
  for (const [key, cls] of Object.entries(exported())) {
    assert.equal(cls.prototype[Symbol.dispose], Symbol.dispose && cls.prototype.dispose, key);
  }
});

test('after dispose(), every member throws Disposed', () => {
  for (const [key, it] of Object.entries(everyClass())) {
    const name = key.split('.')[1];
    const { getters, methods } = members(Object.getPrototypeOf(it));
    assert.equal(it.disposed, false, key);
    for (const g of getters) it[g];
    (Symbol.dispose ? it[Symbol.dispose] : it.dispose).call(it);
    assert.equal(it.disposed, true, key);
    for (const g of getters) {
      assert.throws(() => it[g], disposedError(name), `${key}.${g}`);
    }
    // A method may need arguments, and napi refuses missing ones before the
    // call: what it must not do is answer.
    let refused = 0;
    for (const m of methods) {
      assert.throws(() => it[m](), `${key}.${m}()`);
      try {
        it[m]();
      } catch (e) {
        refused += e.code === 'Disposed';
      }
    }
    assert.ok(refused > 0 || methods.length === 0, `${key}: no method threw Disposed`);
    assert.equal(inspect(it), `${name} { disposed: true }`, key);
    assert.equal(Object.prototype.toString.call(it), `[object ${name}]`, key);
    it.dispose();
    assert.equal(it.disposed, true, key);
  }
});

test('a disposed instance is refused as an argument', () => {
  const tree = usvg.Tree.fromStr(SVG);
  const pixmap = new tiny_skia.Pixmap(10, 10);
  const transform = tiny_skia.Transform.identity();
  transform.dispose();
  assert.throws(() => resvg.render(tree, transform, pixmap), disposedError('Transform'));
  pixmap.dispose();
  assert.throws(() => resvg.render(tree, tiny_skia.Transform.identity(), pixmap), disposedError('Pixmap'));
  tree.dispose();
  assert.throws(() => resvg.render(tree, tiny_skia.Transform.identity(), new tiny_skia.Pixmap(1, 1)), disposedError('Tree'));
  // A union's value, disposed, is refused for the union.
  const node = usvg.Tree.fromStr(SVG).nodeById('r');
  node.value.dispose();
  assert.throws(
    () => resvg.renderNode(node, tiny_skia.Transform.identity(), new tiny_skia.Pixmap(1, 1)),
    disposedError('Path'),
  );
  // A provider field reads its instance when the options convert.
  const db = new fontdb.Database();
  db.dispose();
  assert.throws(() => usvg.Tree.fromStr(SVG, { fontdb: db }), disposedError('Database'));
});

test('a handle drops its own reference: the tree lives on for the others', () => {
  const tree = usvg.Tree.fromStr(SVG);
  const a = tree.root();
  const b = tree.root();
  const node = tree.nodeById('r');
  const children = b.children().length;
  a.dispose();
  assert.throws(() => a.children(), disposedError('Group'));
  assert.equal(b.children().length, children);
  // The tree's own handle gone: what was reached from it still holds it.
  tree.dispose();
  assert.throws(() => tree.root(), disposedError('Tree'));
  assert.equal(b.id(), '');
  assert.equal(node.value.id(), 'r');
  const pixmap = new tiny_skia.Pixmap(20, 20);
  assert.equal(resvg.renderNode(node, tiny_skia.Transform.identity(), pixmap), true);
  // Copy on write: a disposed handle never touches what another holds.
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const parsed = usvg.Tree.fromStr(SVG, { fontdb: db });
  db.dispose();
  assert.equal(parsed.nodeById('t').value.chunks().length, 1);
});

test('dispose() frees a pixmap now, not at garbage collection', () => {
  // 32 pixmaps of 16 MiB, written so their pages are real, each kept alive
  // by the array: without dispose() that is 512 MiB.
  const kept = [];
  const before = process.memoryUsage().rss;
  const white = tiny_skia.Color.fromRgba8(255, 255, 255, 255);
  for (let i = 0; i < 32; i++) {
    const p = new tiny_skia.Pixmap(2048, 2048);
    p.fill(white);
    p.dispose();
    kept.push(p);
  }
  const grown = (process.memoryUsage().rss - before) / 2 ** 20;
  assert.ok(grown < 192, `rss grew ${grown.toFixed(0)} MiB`);
  assert.equal(kept.length, 32);
});

test('without Symbol.dispose (Node < 20.4), the module loads and leaves the alias out', () => {
  const index = fileURLToPath(new URL('../../index.js', import.meta.url));
  const child = `
    // Symbol.dispose cannot be deleted: a stand-in Symbol without it.
    const real = Symbol;
    const fake = function Symbol(description) { return real(description); };
    const own = Object.getOwnPropertyDescriptors(real);
    for (const k of ['dispose', 'asyncDispose', 'prototype', 'length', 'name']) delete own[k];
    Object.defineProperties(fake, own);
    globalThis.Symbol = fake;
    const b = require(${JSON.stringify(index)});
    const p = new b.tiny_skia.Pixmap(1, 1);
    const aliased = (c) => Object.getOwnPropertySymbols(c.prototype).includes(real.dispose);
    p.dispose();
    process.stdout.write(JSON.stringify({
      alias: aliased(b.tiny_skia.Pixmap) || aliased(b.usvg.Tree),
      disposed: p.disposed,
    }));
  `;
  const out = spawnSync(process.execPath, ['-e', child], { encoding: 'utf8' });
  assert.equal(out.status, 0, out.stderr);
  assert.deepEqual(JSON.parse(out.stdout), { alias: false, disposed: true });
});
