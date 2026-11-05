// The option structs as plain objects (ADR 0001; step 5 of
// docs/migration-0.10.md): `usvg.Options` and `usvg.WriteOptions`, every field
// optional over usvg's `Default`. Their shape in index.d.ts, then what
// `usvg.Tree.fromStr` and `tree.toString` make of them.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';
import { declarations } from '../../scripts/api-diff.mjs';

const { usvg, fontdb, tiny_skia, resvg } = binding;
const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));
const TEXT = `<svg xmlns="http://www.w3.org/2000/svg" width="80" height="30">
  <text x="2" y="22" font-family="IBM Plex Sans" font-size="20">Hi</text></svg>`;

/** Whether rendering `tree` at its size leaves any pixel drawn. */
function draws(tree) {
  const size = tree.size();
  const pixmap = new tiny_skia.Pixmap(Math.ceil(size.width()), Math.ceil(size.height()));
  resvg.render(tree, tiny_skia.Transform.identity(), pixmap);
  return pixmap.data().some((v) => v !== 0);
}

const dts = declarations(readFileSync(new URL('../../index.d.ts', import.meta.url), 'utf8'));

/** `{ field: signature }` of an interface `index.d.ts` declares. */
function fields(name) {
  assert.equal(dts.get(name), 'interface', `${name} is an interface`);
  const out = {};
  for (const [key, sig] of dts) {
    const rest = key.startsWith(`${name}.`) && key.slice(name.length + 1);
    if (rest && !rest.includes('.')) out[rest] = sig;
  }
  return out;
}

test('usvg.Options and usvg.WriteOptions: every field optional', () => {
  for (const name of ['usvg.Options', 'usvg.WriteOptions']) {
    const f = fields(name);
    assert.ok(Object.keys(f).length > 5, `${name} has its fields`);
    for (const [field, sig] of Object.entries(f)) {
      assert.match(sig, /^\?: /, `${name}.${field} is optional`);
    }
  }
});

test('every public field of the upstream structs, under its Rust name', () => {
  // usvg 0.49's `Options` and `WriteOptions`, camelCased: a field that stops
  // mapping shows up here, and in the codegen report.
  assert.deepEqual(Object.keys(fields('usvg.Options')).sort(), [
    'defaultSize', 'dpi', 'fontFamily', 'fontResolver', 'fontSize', 'fontdb', 'imageHrefResolver',
    'imageRendering', 'languages', 'resourcesDir', 'shapeRendering', 'styleSheet', 'textRendering',
  ]);
  assert.deepEqual(Object.keys(fields('usvg.WriteOptions')).sort(), [
    'attributesIndent', 'coordinatesPrecision', 'idPrefix', 'indent', 'preserveText',
    'transformsPrecision', 'useSingleQuote',
  ]);
});

test('usvg.Options: fontdb, the resolvers and defaultSize are provider fields', () => {
  assert.equal(fields('usvg.Options').fontdb, '?: fontdb.Database');
  // `default_size` is a `tiny_skia::Size` (Q39): no `fromObject`, its
  // fields are private, so JS builds one with `tiny_skia.Size.fromWh`.
  assert.equal(fields('usvg.Options').defaultSize, '?: tiny_skia.Size');
  // The resolvers take the `guard` providers (test/api/guard.mjs).
  assert.equal(fields('usvg.Options').imageHrefResolver, '?: guard.ImageHrefResolver');
  assert.equal(fields('usvg.Options').fontResolver, '?: guard.FontResolver');
});

test('enum fields are the namespaced enums, indent its JS form', () => {
  const o = fields('usvg.Options');
  assert.equal(o.shapeRendering, '?: usvg.ShapeRendering');
  assert.equal(o.textRendering, '?: usvg.TextRendering');
  assert.equal(o.imageRendering, '?: usvg.ImageRendering');
  const w = fields('usvg.WriteOptions');
  assert.equal(w.indent, "?: number | 'tabs' | 'none'");
  assert.equal(w.attributesIndent, "?: number | 'tabs' | 'none'");
});

test('option objects are types only: nothing at runtime', () => {
  assert.equal(binding.usvg.Options, undefined);
  assert.equal(binding.usvg.WriteOptions, undefined);
});

test('left out, the options are usvg::Options::default(): an empty font database', () => {
  // As in Rust (Q27): no font, so the text renders nothing.
  for (const tree of [usvg.Tree.fromStr(TEXT), usvg.Tree.fromStr(TEXT, {}), usvg.Tree.fromStr(TEXT, { fontdb: undefined })]) {
    assert.equal(tree.fontdb().len(), 0);
    assert.equal(draws(tree), false);
  }
});

test('the fontdb field takes a fontdb.Database, shared with the tree', () => {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const tree = usvg.Tree.fromStr(TEXT, { fontdb: db });
  assert.equal(tree.fontdb().len(), db.len());
  assert.ok(draws(tree), 'the text renders with the font');
  // A provider field takes its class only.
  assert.throws(() => usvg.Tree.fromStr(TEXT, { fontdb: {} }));
  assert.throws(() => usvg.Tree.fromStr(TEXT, { fontdb: tiny_skia.Size.fromWh(1, 1) }));
  // And `defaultSize` a `tiny_skia.Size` only (test/options.mjs: what usvg
  // does with it).
  assert.equal(usvg.Tree.fromStr(TEXT, { defaultSize: tiny_skia.Size.fromWh(40, 25) }).size().width(), 80);
  assert.throws(() => usvg.Tree.fromStr(TEXT, { defaultSize: { width: 40, height: 25 } }));
  const gone = tiny_skia.Size.fromWh(1, 1);
  gone.dispose();
  assert.throws(() => usvg.Tree.fromStr(TEXT, { defaultSize: gone }), { code: 'Disposed' });
});

test('one field changes that field only', () => {
  // An inch at usvg's default dpi (96) and at a set one.
  const inch = '<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="2in"/>';
  const size = (o) => {
    const s = usvg.Tree.fromStr(inch, o).size();
    return [s.width(), s.height()];
  };
  assert.deepEqual(size(), [96, 192]);
  assert.deepEqual(size({ dpi: 72 }), [72, 144]);
  assert.deepEqual(size({ dpi: undefined }), [96, 192]);
  // `font-size="2em"` against the default font size (12) and a set one.
  const em = '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><text font-size="2em">x</text></svg>';
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const out = (o) => usvg.Tree.fromStr(em, { fontdb: db, fontFamily: 'IBM Plex Sans', ...o }).toString({ preserveText: true });
  assert.match(out({}), /font-size="24"/);
  assert.match(out({ fontSize: 10 }), /font-size="20"/);
});

test('WriteOptions: indent, precision and quotes reach the writer', () => {
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
    <g opacity="0.5"><path d="M 0.123456 0 L 10 10" stroke="black"/></g></svg>`);
  const byDefault = tree.toString();
  assert.equal(tree.toString({}), byDefault);
  assert.match(byDefault, /\n {4}<g/, 'usvg indents by 4 spaces');
  assert.match(tree.toString({ indent: 2 }), /\n {2}<g/);
  assert.match(tree.toString({ indent: 'tabs' }), /\n\t<g/);
  assert.doesNotMatch(tree.toString({ indent: 'none' }), /\n/);
  assert.match(byDefault, /M 0\.123456 0/);
  assert.match(tree.toString({ coordinatesPrecision: 2 }), /M 0\.12 0/);
  // Clamped to usvg's table, never an index past it.
  assert.match(tree.toString({ coordinatesPrecision: 200 }), /M 0\.123456 0/);
  assert.match(tree.toString({ useSingleQuote: true }), /width='10'/);
  assert.throws(() => tree.toString({ indent: 'wide' }), /invalid indent/);
});
