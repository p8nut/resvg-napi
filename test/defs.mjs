// Generated classes over usvg's Arc-held definitions, reached from the
// tree's tables (`usvg.Tree`), and fontdb's faces.
import assert from 'node:assert/strict';
import { fontsOrSkip } from './support.mjs';
import binding from '../index.js';
const { usvg, tiny_skia } = binding;
// Fonts are not a given: WASI has none, so the database is explicit and the
// family is whatever this environment actually holds.
const { db, family } = fontsOrSkip(binding, 'definition classes');


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
const tree = usvg.Tree.fromStr(svg, { fontdb: db, fontFamily: family });

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
assert.ok(lg.transform() instanceof tiny_skia.Transform);
assert.deepEqual(Object.keys(lg.transform().toObject()), ['sx', 'kx', 'ky', 'sy', 'tx', 'ty']);
assert.equal(lg.spreadMethod(), 'pad', 'enum mirrored from a wrapper accessor');

// 4. filter region, and the mask/pattern metadata
assert.ok(tree.filters()[0].rect() instanceof tiny_skia.NonZeroRect);
assert.equal(typeof tree.filters()[0].rect().width(), 'number');
assert.equal(tree.masks()[0].kind(), 'luminance');
assert.equal(typeof tree.patterns()[0].rect().width(), 'number');

// 5. generated value objects: stops carry a nested Color, faces come from an
//    iterator. A primitive holds a `tiny_skia.NonZeroRect`, which no plain
//    object can: it is a read-only class of getters.
const stops = lg.stops();
assert.deepEqual(stops.map((s) => s.offset), [0, 1]);
assert.deepEqual(stops[0].color, { red: 255, green: 0, blue: 0 });
assert.deepEqual(stops.map((s) => s.opacity), [1, 1]);
const prims = tree.filters()[0].primitives();
assert.equal(prims.length, 1);
assert.ok(prims[0] instanceof usvg.Primitive);
assert.equal(typeof prims[0].rect.width(), 'number');
assert.equal(prims[0].colorInterpolation, 'linearRgb');
const faces = tree.fontdb().faces();
assert.ok(faces.length > 0);
// FaceInfo maps only partially (Source has no JS form), so the strict
// policy makes it a class with getters rather than a plain object.
//
// `stretch` arrived when the generator started reading a crate's public surface
// the way the crate publishes it: fontdb lifts it out of a private module with
// `pub use ttf_parser::Width as Stretch`, and the old rule -- keep a module or
// drop it whole -- could only drop it.
const plumbing = ['constructor', 'dispose', 'disposed'];
assert.deepEqual(
  Object.getOwnPropertyNames(Object.getPrototypeOf(faces[0])).filter((k) => !plumbing.includes(k)).sort(),
  ['families', 'id', 'index', 'monospaced', 'postScriptName', 'stretch', 'style', 'weight'],
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

// 7. the font database a parse actually resolved
assert.ok(tree.fontdb().len() > 0, 'resolved fontdb reachable from the tree');

// --- fontdb.FaceInfo ----------------------------------------------------------
{
  // 8. faces() hands out class instances, not plain objects
  const face = db.faces()[0];
  assert.ok(face instanceof binding.fontdb.FaceInfo);
  assert.equal(typeof face.postScriptName, 'string');
  // fontdb's own enum, not a string usvg invented: a face is one of three.
  assert.ok(['normal', 'italic', 'oblique'].includes(face.style), `style: ${face.style}`);
  // Family names matter: fontdb resolves by family, not by PostScript name, so
  // `postScriptName` alone leaves a caller unable to fill `fontFamily`.
  // `Vec<(String, Language)>` in Rust: `[name, language]` pairs in JS (Q38),
  // English (US) first when the font has it.
  assert.ok(Array.isArray(face.families) && face.families.length > 0);
  const [[name, language]] = face.families;
  assert.equal(typeof name, 'string');
  assert.ok(Object.values(binding.fontdb.Language).includes(language), `language: ${language}`);
  assert.ok(db.faces().some((f) => f.families.some(([, l]) => l === 'englishUnitedStates')));
  assert.ok(db.faces().some((f) => f.families.some(([n]) => n === family)), 'the family we ask for');
  assert.equal(typeof face.weight, 'number');
  assert.equal(typeof face.index, 'number');
  assert.equal(typeof face.monospaced, 'boolean');
  // `query`, `familyName` and the face `ID` are back through generic rules
  // (#181; test/api/fontdb.mjs); `faceSource` is not yet.
  assert.equal(typeof db.query, 'function');
  assert.equal(db.faceSource, undefined);
}

// 9. a missing font file is an error from the OS. Neither half of the
//    message is portable. Three environments, three answers for the same
//    ENOENT:
//
//      linux native   No such file or directory (os error 2)
//      windows native The system cannot find the file specified. (os error 2)
//      wasm32-wasip1  No such file or directory (os error 44)
//
//    The text differs, and so does the number -- wasi-libc numbers ENOENT 44.
//    What holds everywhere is that the failure came from the OS rather than
//    from the font parser, which is all this check is for.
assert.throws(() => db.loadFontFile('/nope.ttf'), /os error \d+/);

// `tiny_skia.Transform`'s fields are not in the order of SVG's `matrix(...)`
// (the 0.9 `Matrix` doc used to claim they were): tiny-skia says so itself -- "we are using column-major-column-vector
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
// a read-only copy out of `usvg.Filter`, and its `root()` is left for later
// (#191). The kinds still read back.
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
