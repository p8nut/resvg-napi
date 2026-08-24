// Generated wrapper classes over usvg's Arc-held definitions.
import assert from 'node:assert/strict';
import { testFonts, skip } from './test-support.mjs';
import { createRequire } from 'node:module';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('./index.js');
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds.
const fontsFound = testFonts(FontDatabase);
if (!fontsFound) {
  skip('definition classes', 'no font in this environment');
  process.exit(0);
}
const { db, family } = fontsFound;
const opts = { fontFamily: family };


const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
  <defs>
    <linearGradient id="lg" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
    </linearGradient>
    <radialGradient id="rg"><stop offset="0" stop-color="lime"/><stop offset="1" stop-color="teal"/></radialGradient>
    <clipPath id="cp"><rect width="50" height="50"/></clipPath>
    <mask id="mk"><rect width="60" height="50" fill="white"/></mask>
    <pattern id="pt" width="10" height="10" patternUnits="userSpaceOnUse"><circle cx="5" cy="5" r="3" fill="black"/></pattern>
    <filter id="ft"><feGaussianBlur stdDeviation="2"/></filter>
  </defs>
  <rect width="40" height="40" fill="url(#lg)" clip-path="url(#cp)"/>
  <rect x="45" width="20" height="40" fill="url(#rg)" mask="url(#mk)"/>
  <rect x="70" width="25" height="40" fill="url(#pt)" filter="url(#ft)"/>
</svg>`;
const r = new Resvg(svg, opts, db);

// 1. the defs tables are reachable as class instances
assert.deepEqual(r.linearGradients().map((g) => g.id()), ['lg']);
assert.deepEqual(r.radialGradients().map((g) => g.id()), ['rg']);
assert.deepEqual(r.clipPaths().map((c) => c.id()), ['cp']);
assert.deepEqual(r.masks().map((m) => m.id()), ['mk']);
assert.deepEqual(r.patterns().map((p) => p.id()), ['pt']);
assert.deepEqual(r.filters().map((f) => f.id()), ['ft']);

// 2. accessors of the type itself
const [lg] = r.linearGradients();
// usvg keeps the gradient in its own 0..1 space and puts the mapping in the
// transform, so x2 stays 1 and the scale shows up in transform().sx
assert.deepEqual([lg.x1(), lg.y1(), lg.x2(), lg.y2()], [0, 0, 1, 0]);
assert.equal(lg.transform().sx, 40);
const [rg] = r.radialGradients();
assert.equal(typeof rg.r(), 'number');

// 3. inherited through `impl Deref for LinearGradient { Target = BaseGradient }`
assert.equal(lg.id(), 'lg');
assert.deepEqual(Object.keys(lg.transform()), ['sx', 'kx', 'ky', 'sy', 'tx', 'ty']);
assert.equal(lg.spreadMethod(), 'pad', 'enum mirrored from a wrapper accessor');

// 4. filter region, and the mask/clip metadata
assert.equal(typeof r.filters()[0].rect().width, 'number');
assert.equal(r.masks()[0].kind(), 'luminance');
assert.equal(typeof r.patterns()[0].rect().width, 'number');

// 5. generated value objects: stops carry a nested Color, faces come from an iterator
const stops = lg.stops();
assert.deepEqual(stops.map((s) => s.offset), [0, 1]);
assert.deepEqual(stops[0].color, { red: 255, green: 0, blue: 0 });
assert.deepEqual(stops.map((s) => s.opacity), [1, 1]);
const prims = r.filters()[0].primitives();
assert.equal(prims.length, 1);
assert.equal(typeof prims[0].rect.width, 'number');
assert.equal(prims[0].colorInterpolation, 'linearRgb');
const faces = r.fontdb().faces();
assert.ok(faces.length > 0);
// FaceInfo maps only partially (ID, Source, Style, Stretch have no JS form), so
// the strict policy makes it a class with getters rather than a plain object.
assert.deepEqual(
  Object.getOwnPropertyNames(Object.getPrototypeOf(faces[0])).sort(),
  ['constructor', 'families', 'index', 'monospaced', 'postScriptName', 'weight'],
);

// 6. a def's content is walkable as nodes, rooted at that def
const pat = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
  <defs><pattern id="p" width="10" height="10" patternUnits="userSpaceOnUse">
    <circle id="dot" cx="5" cy="5" r="3"/><rect id="bar" width="10" height="2"/>
  </pattern></defs><rect width="40" height="40" fill="url(#p)"/></svg>`).patterns()[0];
const inside = pat.children();
assert.deepEqual(inside.map((n) => [n.kind, n.id()]), [['path', 'dot'], ['path', 'bar']]);
assert.deepEqual(Object.values(inside[0].absBoundingBox()), [2, 2, 6, 6]);
assert.equal(inside[0].clipPath(), null, 'no document context through a def');

// 7. font bytes, keyed by PostScript name
const resolved = r.fontdb();
const face = resolved.faces()[0];
const bytes = resolved.faceData(face.postScriptName);
assert.ok(bytes.length > 1000);
assert.match(bytes.subarray(0, 4).toString('hex'), /^(4f54544f|00010000|74727565|74746366)$/, 'sfnt magic');
assert.equal(resolved.faceData('Nope-Regular'), null);

// 8. the font database a parse actually resolved
assert.ok(r.fontdb().len() > 0, 'resolved fontdb reachable from the tree');

console.log('ok — generated definition classes + value objects: all checks passed');

// --- FontFace: the value class that unlocked the ID-gated fontdb methods ------
{
  const before = db.len();

  // 9. faces() hands out class instances, not plain objects
  const face = db.faces()[0];
  assert.equal(typeof face.postScriptName, 'string');
  // Family names matter: fontdb resolves by family, not by PostScript name, so
  // `postScriptName` alone leaves a caller unable to fill `fontFamily`.
  assert.ok(Array.isArray(face.families) && face.families.length > 0);
  assert.equal(typeof face.families[0], 'string');
  assert.equal(typeof face.weight, 'number');
  assert.equal(typeof face.index, 'number');
  assert.equal(typeof face.monospaced, 'boolean');

  // 10. a FontFace stands in for the opaque fontdb ID
  assert.equal(db.face(face).postScriptName, face.postScriptName, 'round trip through face()');

  // 11. query: a CSS-ish request resolves to a face. Before the removal below,
  // because a database with a single face has nothing left to match afterwards
  // -- which is exactly the WASI case, where the only face is the loaded file.
  const hit = db.query([face.families[0]], 400, false);
  assert.ok(hit && hit.postScriptName.length > 0);
  assert.ok(db.query(['sans-serif']), 'generic families are understood');
  assert.equal(db.query(['No Such Family At All']), null);

  // 12. bytes still reachable, keyed by PostScript name
  const data = db.faceData(hit.postScriptName);
  assert.match(data.subarray(0, 4).toString('hex'), /^(4f54544f|00010000|74727565|74746366)$/);

  // 13. removal, by the same class: the id never leaves Rust
  db.removeFace(face);
  assert.equal(db.len(), before - 1, 'removeFace took the id from the class');
  assert.equal(db.face(face), null, 'and it is gone');
}
console.log('ok — FontFace + strict objects: all checks passed');
