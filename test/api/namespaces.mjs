// The second surface: one namespace per crate (`usvg.*`, `fontdb.*`), Rust
// names, beside the flat exports it will replace. Nothing in it can be built
// from JS yet -- factories come with step 6 of docs/migration-0.10.md -- so
// this checks its shape against index.d.ts: every class and enum declared
// is there at runtime, under its Rust name, with the members declared.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';
import { declarations } from '../../scripts/api-diff.mjs';

const dts = declarations(readFileSync(new URL('../../index.d.ts', import.meta.url), 'utf8'));
const NAMESPACES = ['usvg', 'fontdb'];

/** `{ classes, enums, types }` a namespace declares, by name. */
function declared(ns) {
  const out = { classes: new Map(), enums: new Map(), types: new Set() };
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

test('every class and enum declared is there, under its Rust name', () => {
  for (const ns of NAMESPACES) {
    const { classes, enums } = declared(ns);
    assert.ok(enums.size > 0, `${ns} declares enums`);
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
    const known = new Set([...classes.keys(), ...enums.keys()]);
    for (const name of Object.keys(binding[ns])) {
      assert.ok(known.has(name), `${ns}.${name} is declared as a class or an enum`);
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
      assert.doesNotMatch(name, /^(Usvg|Fontdb)/, `${ns}.${name}`);
    }
  }
  // A type is in the namespace of the crate that defines it: fontdb's face
  // style is not usvg's, though usvg re-exports fontdb.
  assert.equal(usvg.Style, undefined);
  assert.equal(usvg.Database, undefined);
  assert.equal(typeof usvg.FontStyle, 'object');
});

test('no namespaced class shares a name with a flat one', () => {
  // napi registers one constructor per class name, namespace or not: a
  // second class of the same name would hand out instances of the first.
  // The generator leaves such a class out until the flat one goes.
  for (const ns of NAMESPACES) {
    for (const [name] of declared(ns).classes) {
      assert.equal(binding[name], undefined, `${ns}.${name} beside a flat ${name}`);
    }
  }
  const db = new binding.FontDatabase();
  assert.ok(db instanceof binding.FontDatabase, 'the flat classes are unchanged');
  assert.ok(!(db instanceof binding.fontdb.Database));
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
  // `Database::new` is an associated function: no constructor until step 6.
  assert.throws(() => new Database());
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
  for (const t of ['Color', 'Fill', 'Stroke', 'Path', 'Stop', 'PaintColor', 'KindBlend', 'TextDecoration']) {
    assert.ok(types.has(t), `usvg.${t} declared`);
    assert.equal(binding.usvg[t], undefined, `usvg.${t} is a type, not a value`);
  }
});
