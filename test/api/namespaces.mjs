// The second surface: one namespace per crate (`usvg.*`, `fontdb.*`,
// `tiny_skia.*`, `svgtypes.*`, `resvg.*`), Rust names, beside the flat exports
// it will replace. This checks its shape against index.d.ts: every class,
// enum and function declared is there at runtime, under its Rust name, with
// the members declared. What they do is for render.mjs, errors.mjs and
// conversions.mjs beside it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';
import { inspect } from 'node:util';
import { declarations } from '../../scripts/api-diff.mjs';
import { testFonts } from '../support.mjs';

const dts = declarations(readFileSync(new URL('../../index.d.ts', import.meta.url), 'utf8'));
const NAMESPACES = ['usvg', 'fontdb', 'tiny_skia', 'svgtypes', 'resvg'];

/** `{ classes, enums, functions, types }` a namespace declares, by name. */
function declared(ns) {
  const out = { classes: new Map(), enums: new Map(), functions: new Set(), types: new Set() };
  for (const [key, sig] of dts) {
    const [head, name, member, ...more] = key.split('.');
    if (head !== ns || !name || more.length) continue;
    if (member) {
      if (out.classes.has(name)) out.classes.get(name).push(member);
      continue;
    }
    // A string enum is declared as a union of its values.
    const values = sig.match(/^type = ((?:'[^']*'(?: \| )?)+)$/);
    if (sig.startsWith('class')) out.classes.set(name, []);
    else if (sig.startsWith('function')) out.functions.add(name);
    else if (values) out.enums.set(name, [...values[1].matchAll(/'([^']*)'/g)].map((m) => m[1]));
    else out.types.add(name);
  }
  return out;
}

test('each crate is a namespace export', () => {
  for (const ns of NAMESPACES) {
    assert.equal(typeof binding[ns], 'object', ns);
    assert.ok(dts.has(ns), `${ns} declared in index.d.ts`);
  }
});

test('every class, enum and function declared is there, under its Rust name', () => {
  for (const ns of NAMESPACES) {
    const { classes, enums, functions } = declared(ns);
    assert.ok(classes.size + functions.size > 0, `${ns} declares classes or functions`);
    for (const name of functions) assert.equal(typeof binding[ns][name], 'function', `${ns}.${name}`);
    for (const [name] of classes) {
      assert.equal(typeof binding[ns][name], 'function', `${ns}.${name}`);
      assert.equal(binding[ns][name].name, name, `${ns}.${name}: the class's own name`);
    }
    for (const [name, values] of enums) {
      // napi declares the string enums as unions, and exports an object of
      // them too: `{ NonZero: 'nonZero', ... }`.
      const e = binding[ns][name];
      assert.equal(typeof e, 'object', `${ns}.${name}`);
      assert.deepEqual(Object.values(e).sort(), [...values].sort(), `${ns}.${name} values`);
    }
    // What runs is what is declared: no runtime export the d.ts does not name.
    const known = new Set([...classes.keys(), ...enums.keys(), ...functions]);
    for (const name of Object.keys(binding[ns])) {
      assert.ok(known.has(name), `${ns}.${name} is declared as a class, an enum or a function`);
    }
  }
});

test('the Rust names, not the flat surface\'s renames', () => {
  const { fontdb, usvg } = binding;
  assert.equal(typeof fontdb.FaceInfo, 'function', 'fontdb.FaceInfo, not FontFace');
  assert.equal(fontdb.FontFace, undefined);
  assert.equal(typeof fontdb.Style, 'object', 'fontdb.Style, not FontFaceStyle');
  assert.equal(fontdb.FontFaceStyle, undefined);
  assert.equal(typeof fontdb.Database, 'function', 'fontdb.Database, not FontDatabase');
  assert.deepEqual({ ...fontdb.Style }, { ...binding.FontFaceStyle });
  assert.deepEqual({ ...usvg.FillRule }, { ...binding.FillRule });
  // The crate prefix is the Rust identifier only. napi still declares it as a
  // type alias of the class (`fontdb.FontdbDatabase`), never as a value.
  for (const ns of NAMESPACES) {
    for (const name of Object.keys(binding[ns])) {
      assert.doesNotMatch(name, /^(Usvg|Fontdb|TinySkia|Svgtypes|Resvg)/, `${ns}.${name}`);
    }
  }
  // A type is in the namespace of the crate that defines it: fontdb's face
  // style is not usvg's, though usvg re-exports fontdb.
  assert.equal(usvg.Style, undefined);
  assert.equal(usvg.Database, undefined);
  assert.equal(typeof usvg.FontStyle, 'object');
});

// The usvg classes whose flat copy was renamed `Legacy*` so they could be
// namespaced (decision Q36; the flat ones go in step 13).
const LEGACY = [
  'ClipPath', 'Filter', 'Font', 'Image', 'ImageKindGif', 'ImageKindJpeg', 'ImageKindPng',
  'ImageKindWebp', 'LinearGradient', 'Mask', 'Pattern', 'PositionedGlyph', 'RadialGradient',
  'Span', 'Text', 'TextChunk', 'TextSpan',
];

test('no namespaced class shares a name with a flat one', () => {
  // napi registers one constructor per class name, namespace or not: a
  // second class of the same name would hand out instances of the first.
  // The generator renames the flat one `Legacy*` instead.
  for (const ns of NAMESPACES) {
    for (const [name] of declared(ns).classes) {
      assert.equal(binding[name], undefined, `${ns}.${name} beside a flat ${name}`);
    }
  }
  const db = new binding.FontDatabase();
  assert.ok(db instanceof binding.FontDatabase, 'the flat classes are unchanged');
  assert.ok(!(db instanceof binding.fontdb.Database));
});

test('the usvg classes are namespaced, their flat copies renamed Legacy*', () => {
  const { classes } = declared('usvg');
  for (const name of LEGACY) {
    assert.ok(classes.has(name), `usvg.${name} declared`);
    assert.equal(typeof binding.usvg[name], 'function', `usvg.${name}`);
    assert.equal(typeof binding[`Legacy${name}`], 'function', `flat Legacy${name}`);
    assert.equal(binding[`Legacy${name}`].name, `Legacy${name}`);
    assert.notEqual(binding[`Legacy${name}`], binding.usvg[name]);
  }
});

test('flat Legacy* instances still work beside their namespaced twins', () => {
  const fonts = testFonts(binding.FontDatabase);
  // A 1x1 PNG, rendered here, for an <image> node.
  const png = new binding.Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>').renderPng();
  const r = new binding.Resvg(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="50">
    <defs>
      <linearGradient id="lg"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
      <radialGradient id="rg"><stop offset="0" stop-color="lime"/><stop offset="1" stop-color="teal"/></radialGradient>
      <clipPath id="cp"><rect width="50" height="50"/></clipPath>
      <mask id="mk"><rect width="60" height="50" fill="white"/></mask>
      <pattern id="pt" width="10" height="10" patternUnits="userSpaceOnUse"><rect width="5" height="5"/></pattern>
      <filter id="ft"><feGaussianBlur stdDeviation="2"/></filter>
    </defs>
    <g id="a" clip-path="url(#cp)"><rect width="40" height="40" fill="url(#lg)"/></g>
    <g id="b" mask="url(#mk)"><rect x="45" width="20" height="40" fill="url(#rg)"/></g>
    <rect x="70" width="25" height="40" fill="url(#pt)" filter="url(#ft)"/>
    <image id="im" width="1" height="1" xlink:href="data:image/png;base64,${png.toString('base64')}"/>
    ${fonts ? '<text id="t" x="5" y="45" font-size="10">Hi</text>' : ''}
  </svg>`, fonts ? { fontFamily: fonts.family } : {}, fonts?.db);
  // usvg wraps an image in a group of its own: the image is its child.
  const image = r.node('im').children()[0].image();
  const got = {
    ClipPath: [r.node('a').clipPath(), ...r.clipPaths()],
    Mask: [r.node('b').mask(), ...r.masks()],
    LinearGradient: r.linearGradients(),
    RadialGradient: r.radialGradients(),
    Pattern: r.patterns(),
    Filter: r.filters(),
    Image: [image],
    ImageKindPng: [image.kind],
  };
  if (fonts) {
    const text = r.node('t').text();
    const span = text.chunks[0].spans[0];
    Object.assign(got, {
      Text: [text], TextChunk: text.chunks, TextSpan: [span], Font: [span.font],
      Span: text.layouted, PositionedGlyph: text.layouted[0].positionedGlyphs,
    });
  }
  for (const [name, values] of Object.entries(got)) {
    assert.ok(values.length > 0, name);
    for (const v of values) {
      assert.equal(v.constructor.name, `Legacy${name}`);
      assert.ok(v instanceof binding[`Legacy${name}`], `instanceof Legacy${name}`);
      assert.ok(!(v instanceof binding.usvg[name]), `not a usvg.${name}`);
      assert.equal(Object.prototype.toString.call(v), `[object Legacy${name}]`);
      // Every getter and every method declared without arguments answers:
      // the receiver is the class napi built it from.
      const proto = Object.getPrototypeOf(v);
      for (const [key, d] of Object.entries(Object.getOwnPropertyDescriptors(proto))) {
        if (d.get) assert.doesNotThrow(() => d.get.call(v), `Legacy${name}.${key}`);
        else if (dts.get(`Legacy${name}.${key}`)?.startsWith('()')) {
          assert.doesNotThrow(() => d.value.call(v), `Legacy${name}.${key}()`);
        }
      }
      assert.match(inspect(v), new RegExp(`^Legacy${name} \\{`));
    }
  }
});

test('fontdb.Database: Rust methods in camelCase, the &mut ones included', () => {
  const { Database } = binding.fontdb;
  const methods = declared('fontdb').classes.get('Database');
  for (const m of [
    'loadFontData', 'loadFontFile', 'loadFontsDir', 'loadSystemFonts', 'isEmpty', 'len',
    'setSerifFamily', 'setSansSerifFamily', 'setCursiveFamily', 'setFantasyFamily',
    'setMonospaceFamily', 'faces',
  ]) {
    assert.ok(methods.includes(m), `declared: ${m}`);
    assert.equal(typeof Database.prototype[m], 'function', `fontdb.Database.prototype.${m}`);
  }
  // `Database::new` is the constructor: an empty database.
  const db = new Database();
  assert.ok(db instanceof Database);
  assert.equal(db.len(), 0);
});

test('fontdb.FaceInfo: a read-only class of getters', () => {
  const proto = binding.fontdb.FaceInfo.prototype;
  for (const g of ['index', 'postScriptName', 'style', 'weight', 'stretch', 'monospaced']) {
    const d = Object.getOwnPropertyDescriptor(proto, g);
    assert.equal(typeof d?.get, 'function', `get ${g}`);
    assert.equal(d.set, undefined, `no setter for ${g}`);
  }
});

test('usvg data types and unions are declared as plain types', () => {
  const { types } = declared('usvg');
  for (const t of ['Color', 'Fill', 'Stroke', 'Stop', 'PaintColor', 'KindBlend', 'TextDecoration', 'NodeGroup']) {
    assert.ok(types.has(t), `usvg.${t} declared`);
    assert.equal(binding.usvg[t], undefined, `usvg.${t} is a type, not a value`);
  }
});
