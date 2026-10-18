// The generated surface, read end to end. Most getters come out of the codegen
// and nothing else calls them, so a mapping that breaks -- a panic, a throw, a
// wrong type -- would ship unnoticed. This walks a document that exercises the
// rare members, reads every getter on everything it reaches, and checks the
// ones no other test touches were actually read.
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { testFonts, testFontFile, skip } from './support.mjs';
const require = createRequire(import.meta.url);
const binding = require('../index.js');
const { Resvg, FontDatabase } = binding;

// 1. which artifact is loaded: the native addon, or the WASI module when
//    NAPI_RS_NATIVE_LIBRARY_PATH points at its loader (npm run test:wasi).
const wasi = process.env.NAPI_RS_NATIVE_LIBRARY_PATH?.includes('wasi');
if (wasi) {
  assert.match(binding.__napiBindingTarget, /^wasm32-wasi/, '__napiBindingTarget under WASI');
} else {
  assert.equal(binding.__napiBindingTarget, 'native', '__napiBindingTarget');
}

// 2. FontDatabase members no other test calls
{
  const db = new FontDatabase();
  assert.equal(db.isEmpty(), true, 'a new database is empty');
  const file = testFontFile();
  if (file && wasi) {
    // No mmap under WASI: fontdb is built without `memmap` there, so files
    // are read into memory, and the one call that needs a mapping throws.
    db.loadFontsDir(join(file, '..'));
    assert.equal(db.isEmpty(), false, 'loadFontsDir under WASI');
    const one = new FontDatabase();
    one.loadFontFile(file);
    assert.equal(one.len(), 1, 'loadFontFile under WASI');
    assert.throws(() => one.makeFaceDataUnshared(one.faces()[0]), /makeFaceDataUnshared is not available on this target/);
  } else if (file) {
    const dir = mkdtempSync(join(tmpdir(), 'resvg-fonts-'));
    copyFileSync(file, join(dir, 'face.ttf'));
    db.loadFontsDir(dir);
    assert.equal(db.isEmpty(), false, 'loadFontsDir found the face');
    const [face] = db.faces();
    db.makeFaceDataUnshared(face);
    assert.ok(db.faceData(face.postScriptName)?.length > 0, 'data still readable once unshared');
    db.loadFontsDir(join(dir, 'nowhere'));
    assert.equal(db.len(), 1, 'a missing directory adds nothing and does not throw');
  } else {
    skip('loadFontsDir / makeFaceDataUnshared', 'no font file in this environment');
  }
}

// 3. the walk
const fonts = testFonts(FontDatabase);
if (!fonts) {
  skip('generated getters', 'no font in this environment');
  process.exit(0);
}
const png = readFileSync(new URL('../demo/assets/roster.png', import.meta.url)).toString('base64');
const doc = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="320" height="240">
  <defs>
    <radialGradient id="rg" cx="50%" cy="50%" r="50%" fx="30%" fy="30%"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/></radialGradient>
    <linearGradient id="lg"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>
    <pattern id="pt" width="8" height="8" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="teal"/></pattern>
    <clipPath id="cp"><circle cx="40" cy="40" r="30"/></clipPath>
    <mask id="mk"><rect width="80" height="80" fill="#fff"/></mask>
    <filter id="fl"><feGaussianBlur stdDeviation="2"/><feOffset dx="2" dy="2"/></filter>
    <path id="curve" d="M10 200 Q160 150 310 200"/>
  </defs>
  <g id="grp" opacity=".9" clip-path="url(#cp)" mask="url(#mk)" filter="url(#fl)">
    <rect id="r" width="80" height="80" fill="url(#rg)" stroke="url(#lg)" stroke-width="3" paint-order="stroke"/>
  </g>
  <rect x="90" width="40" height="40" fill="url(#pt)"/>
  <text id="t" x="10 20 30" y="120" dy="0 2 4" font-size="18" letter-spacing="1" word-spacing="3"
        font-variant="small-caps" font-kerning="none" dominant-baseline="middle" alignment-baseline="middle"
        font-variation-settings="'wght' 500" text-decoration="underline overline line-through"
        textLength="200" lengthAdjust="spacingAndGlyphs" paint-order="stroke">Spaced Text</text>
  <text id="v" x="300" y="20" writing-mode="tb" font-size="12">vertical</text>
  <text font-size="14"><textPath xlink:href="#curve">along a path</textPath></text>
  <image id="im" x="160" y="10" width="60" height="40" xlink:href="data:image/png;base64,${png}"/>
</svg>`, { fontFamily: fonts.family }, fonts.db);

// Zero-argument methods are read off index.d.ts, so a method the codegen adds
// is walked too; renders, serialisers and async twins are not reads.
const dts = readFileSync(new URL('../index.d.ts', import.meta.url), 'utf8');
const zeroArg = new Set([...dts.matchAll(/^ {2}(\w+)\(\): /gm)].map((m) => m[1])
  .filter((n) => !/^(render|to|take)|Async$/.test(n)));

const read = new Set();
const seen = new WeakSet();
function walk(value, depth) {
  if (value === null || typeof value !== 'object' || ArrayBuffer.isView(value) || depth > 12) return;
  if (Array.isArray(value)) return value.forEach((v) => walk(v, depth + 1));
  if (seen.has(value)) return;
  seen.add(value);
  const cls = value.constructor?.name;
  if (!cls || cls === 'Object') return Object.values(value).forEach((v) => walk(v, depth + 1));
  for (let p = Object.getPrototypeOf(value); p && p !== Object.prototype; p = Object.getPrototypeOf(p)) {
    for (const [name, d] of Object.entries(Object.getOwnPropertyDescriptors(p))) {
      const call = d.get ? () => d.get.call(value) : zeroArg.has(name) && typeof d.value === 'function' ? () => d.value.call(value) : null;
      if (!call) continue;
      let out;
      assert.doesNotThrow(() => { out = call(); }, `${cls}.${name}`);
      read.add(`${cls}.${name}`);
      walk(out, depth + 1);
    }
  }
}
walk(doc, 0);
for (const id of ['grp', 'r', 't', 'v', 'im']) walk(doc.node(id), 0);

// The members the review found untested anywhere, by class: each must have
// been reached, or the document above stopped exercising it.
const wanted = [
  'TextSpan.letterSpacing', 'TextSpan.wordSpacing', 'TextSpan.smallCaps', 'TextSpan.applyKerning',
  'TextSpan.dominantBaseline', 'TextSpan.alignmentBaseline', 'TextSpan.paintOrder', 'Font.variations',
  'TextChunk.textFlow', 'Text.dy', 'Text.writingMode',
  'PositionedGlyph.outlineTransform', 'PositionedGlyph.svgTransform', 'PositionedGlyph.colrTransform',
  'RadialGradient.fx', 'RadialGradient.fy', 'Image.isVisible',
];
const missed = wanted.filter((m) => !read.has(m));
assert.deepEqual(missed, [], 'getters the walk should have reached');
console.log(`ok — generated surface: ${read.size} members read without a throw`);
