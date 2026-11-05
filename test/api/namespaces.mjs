// The surface: one namespace per crate (`usvg.*`, `fontdb.*`, `tiny_skia.*`,
// `svgtypes.*`, `resvg.*`, and the hand-written `guard.*`), Rust names, and
// nothing at the root but the global logs. This checks its shape against
// index.d.ts: every class, enum and function declared is there at runtime,
// under its Rust name, with the members declared. What they do is for
// render.mjs, errors.mjs and conversions.mjs beside it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';
import { inspect } from 'node:util';
import { declarations } from '../../scripts/api-diff.mjs';

const dts = declarations(readFileSync(new URL('../../index.d.ts', import.meta.url), 'utf8'));
const NAMESPACES = ['usvg', 'fontdb', 'tiny_skia', 'svgtypes', 'resvg', 'guard'];

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

test('the Rust names, and nothing else at the root', () => {
  const { fontdb, usvg } = binding;
  assert.equal(typeof fontdb.FaceInfo, 'function', 'fontdb.FaceInfo, not FontFace');
  assert.equal(fontdb.FontFace, undefined);
  assert.equal(typeof fontdb.Style, 'object', 'fontdb.Style, not FontFaceStyle');
  assert.equal(fontdb.FontFaceStyle, undefined);
  assert.equal(typeof fontdb.Database, 'function', 'fontdb.Database, not FontDatabase');
  assert.deepEqual({ ...fontdb.Style }, { Normal: 'normal', Italic: 'italic', Oblique: 'oblique' });
  // The 0.9 flat surface is gone: the root holds the namespaces and the
  // global log functions only.
  assert.deepEqual(
    Object.keys(binding).filter((k) => !k.startsWith('__')).sort(),
    [...NAMESPACES, 'setLogLevel', 'takeLogs'].sort(),
  );
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

// The usvg classes the flat surface had copies of, under other names until
// those went with the flat node classes (step 7b of the migration).
const USVG_CLASSES = [
  'ClipPath', 'Filter', 'Font', 'Image', 'ImageKindGif', 'ImageKindJpeg', 'ImageKindPng',
  'ImageKindWebp', 'LinearGradient', 'Mask', 'Pattern', 'PositionedGlyph', 'RadialGradient',
  'Span', 'Text', 'TextChunk', 'TextSpan',
];

test('the usvg classes are namespaced, with no flat copy left', () => {
  const { classes } = declared('usvg');
  for (const name of USVG_CLASSES) {
    assert.ok(classes.has(name), `usvg.${name} declared`);
    assert.equal(typeof binding.usvg[name], 'function', `usvg.${name}`);
    assert.equal(binding[name], undefined, `no flat ${name}`);
    assert.equal(binding[`Legacy${name}`], undefined, `no flat Legacy${name}`);
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
  for (const g of ['families', 'index', 'postScriptName', 'style', 'weight', 'stretch', 'monospaced']) {
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
