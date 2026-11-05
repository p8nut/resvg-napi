// The option structs as plain objects (ADR 0001; step 5 of
// docs/migration-0.10.md): `usvg.Options` and `usvg.WriteOptions`, every field
// optional over usvg's `Default`. Nothing namespaced takes them yet --
// `usvg.Tree.fromData` comes with step 6, and the runtime checks with it -- so
// this checks their shape in index.d.ts. The conversion itself (omitted fields
// keep `Default`, a provider field, the clamp) is tested on the toy crate.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';
import { declarations } from '../../scripts/api-diff.mjs';

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

test('the same fields as the flat options, from the same upstream structs', () => {
  const flat = (n) => Object.keys(fields(n)).sort();
  // `resourcesRoot` is the flat surface's own fence, not a usvg field; the
  // font database is a `Resvg` argument there and a field here.
  const render = flat('RenderOptions').filter((f) => f !== 'resourcesRoot');
  assert.deepEqual(Object.keys(fields('usvg.Options')).sort(), [...render, 'fontdb'].sort());
  assert.deepEqual(Object.keys(fields('usvg.WriteOptions')).sort(), flat('WriteOptions'));
});

test('usvg.Options.fontdb takes a fontdb.Database (a provider field)', () => {
  assert.equal(fields('usvg.Options').fontdb, '?: fontdb.Database');
  // The resolvers get their provider classes in step 10 (`guard`): until
  // then they are left out, and usvg's default resolvers apply.
  assert.equal(fields('usvg.Options').imageHrefResolver, undefined);
  assert.equal(fields('usvg.Options').fontResolver, undefined);
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
