// Generated classes over usvg's Arc-held definitions, reached from the
// tree's tables (`usvg.Tree`), and the flat font classes.
import assert from 'node:assert/strict';
import { fontsOrSkip } from './support.mjs';
import binding from '../index.js';
const { Resvg, FontDatabase, usvg } = binding;
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds.
const { db, family } = fontsOrSkip(FontDatabase, 'definition classes');
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
const tree = usvg.Tree.fromStr(svg);

// 1. the defs tables are reachable as class instances (clip paths, masks and
//    patterns: test/api/nodes.mjs)
assert.deepEqual(tree.linearGradients().map((g) => g.id()), ['lg']);
assert.deepEqual(tree.radialGradients().map((g) => g.id()), ['rg']);
assert.deepEqual(tree.filters().map((f) => f.id()), ['ft']);

// 2. accessors of the type itself
const [lg] = tree.linearGradients();
// usvg keeps the gradient in its own 0..1 space and puts the mapping in the
// transform, so x2 stays 1 and the scale shows up in transform().sx
assert.deepEqual([lg.x1(), lg.y1(), lg.x2(), lg.y2()], [0, 0, 1, 0]);
assert.equal(lg.transform().sx, 40);
const [rg] = tree.radialGradients();
assert.equal(typeof rg.r(), 'number');

// 3. inherited through `impl Deref for LinearGradient { Target = BaseGradient }`
assert.equal(lg.id(), 'lg');
assert.deepEqual(Object.keys(lg.transform()), ['sx', 'kx', 'ky', 'sy', 'tx', 'ty']);
assert.equal(lg.spreadMethod(), 'pad', 'enum mirrored from a wrapper accessor');

// 4. filter region, and the mask/pattern metadata
assert.equal(typeof tree.filters()[0].rect().width, 'number');
assert.equal(tree.masks()[0].kind(), 'luminance');
assert.equal(typeof tree.patterns()[0].rect().width, 'number');

// 5. generated value objects: stops carry a nested Color, faces come from an iterator
const stops = lg.stops();
assert.deepEqual(stops.map((s) => s.offset), [0, 1]);
assert.deepEqual(stops[0].color, { red: 255, green: 0, blue: 0 });
assert.deepEqual(stops.map((s) => s.opacity), [1, 1]);
const prims = tree.filters()[0].primitives();
assert.equal(prims.length, 1);
assert.equal(typeof prims[0].rect.width, 'number');
assert.equal(prims[0].colorInterpolation, 'linearRgb');
const faces = r.fontdb().faces();
assert.ok(faces.length > 0);
// FaceInfo maps only partially (ID and Source have no JS form), so the strict
// policy makes it a class with getters rather than a plain object.
//
// `stretch` arrived when the generator started reading a crate's public surface
// the way the crate publishes it: fontdb lifts it out of a private module with
// `pub use ttf_parser::Width as Stretch`, and the old rule -- keep a module or
// drop it whole -- could only drop it.
assert.deepEqual(
  Object.getOwnPropertyNames(Object.getPrototypeOf(faces[0])).sort(),
  ['constructor', 'families', 'index', 'monospaced', 'postScriptName', 'stretch', 'style', 'weight'],
);
// The nine OS/2 width classes, spelled as usvg spells its own enums.
assert.ok(['ultraCondensed', 'condensed', 'normal', 'expanded', 'ultraExpanded', 'semiCondensed',
  'extraCondensed', 'semiExpanded', 'extraExpanded'].includes(faces[0].stretch),
  `unexpected stretch: ${faces[0].stretch}`);

// 6. a def's content is a group of the same tree, so a node inside a pattern
//    resolves the document's tables too (the defs chains themselves:
//    test/api/nodes.mjs)
{
  const nested = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40">
    <defs>
      <clipPath id="inner"><rect width="20" height="20"/></clipPath>
      <pattern id="p" width="10" height="10" patternUnits="userSpaceOnUse">
        <g clip-path="url(#inner)"><circle id="dot" cx="5" cy="5" r="3"/></g><rect id="bar" width="10" height="2"/>
      </pattern>
    </defs>
    <rect width="40" height="40" fill="url(#p)"/>
  </svg>`);
  const inside = nested.patterns()[0].root().children();
  assert.deepEqual(inside.map((n) => n.kind), ['group', 'path']);
  assert.equal(inside[0].value.clipPath()?.id(), 'inner', 'a def node resolves the document tables');
  const dot = inside[0].value.children()[0].value;
  assert.equal(dot.id(), 'dot');
  const b = dot.absBoundingBox();
  assert.deepEqual([b.x(), b.y(), b.width(), b.height()], [2, 2, 6, 6]);
  assert.equal(inside[1].value.id(), 'bar');
}

// 7. font bytes, keyed by PostScript name
const resolved = r.fontdb();
const face = resolved.faces()[0];
const bytes = resolved.faceData(face.postScriptName);
assert.ok(bytes.length > 1000);
assert.match(bytes.subarray(0, 4).toString('hex'), /^(4f54544f|00010000|74727565|74746366)$/, 'sfnt magic');
assert.equal(resolved.faceData('Nope-Regular'), null);

// 8. the font database a parse actually resolved
assert.ok(r.fontdb().len() > 0, 'resolved fontdb reachable from the tree');

// --- FontFace: the value class that unlocked the ID-gated fontdb methods ------
{
  const before = db.len();

  // 9. faces() hands out class instances, not plain objects
  const face = db.faces()[0];
  assert.equal(typeof face.postScriptName, 'string');
  // fontdb's own enum, not a string usvg invented: a face is one of three.
  assert.ok(['normal', 'italic', 'oblique'].includes(face.style), `style: ${face.style}`);
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
// The Matrix doc used to claim its field order was SVG's `matrix(...)`. It is
// not: tiny-skia says so itself -- "we are using column-major-column-vector
// matrix notation, therefore it's ky-kx, not kx-ky". Reading the fields
// positionally mirrors the transform, silently, so this pins which order
// actually round-trips.
{
  const rot = (t) => usvg.Tree.fromStr(
    `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g transform="${t}"><rect id="r" width="10" height="10"/></g></svg>`,
  ).nodeById('r').value.absTransform();

  const m = rot('rotate(30)');
  const near = (a, b) => Math.abs(a - b) < 1e-4;
  const six = (o) => `matrix(${o.join(' ')})`;

  // declaration order is the wrong one, and wrong in a way that looks right
  const declared = rot(six([m.sx, m.kx, m.ky, m.sy, m.tx, m.ty]));
  assert.ok(near(declared.kx, -m.kx), 'positional round trip mirrors the rotation');

  // the order SVG actually takes
  const svgOrder = rot(six([m.sx, m.ky, m.kx, m.sy, m.tx, m.ty]));
  assert.ok(near(svgOrder.kx, m.kx) && near(svgOrder.ky, m.ky), 'sx ky kx sy tx ty round-trips');
}

// feImage: what the primitive references is not reachable yet. A primitive is
// a plain object copied out of `usvg.Filter`, and its `root()` is left for
// later (docs/migration-0.10.md, step 7). The kinds still read back.
{
  const tree = usvg.Tree.fromStr(
    '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><defs>' +
    '<rect id="src" width="5" height="5" fill="red"/>' +
    '<filter id="f"><feImage href="#src"/><feOffset dx="1"/></filter></defs>' +
    '<rect width="20" height="20" filter="url(#f)"/></svg>',
  );
  const [f] = tree.filters();
  assert.deepEqual(f.primitives().map((p) => p.kind.type), ['image', 'offset']);
}
