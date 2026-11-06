// Font resolution: the helpers' `pendingFonts`, read from a fresh
// `guard.FontResolver` per parse (usvg's resolver is Send + Sync, so no JS
// callback; what it noted is read back after the parse).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../index.js';
import { parse, renderRaw } from '../helpers.mjs';
import { fontsOrSkip, testFontFile, skip } from './support.mjs';

const { fontdb } = binding;

const text = (family) => `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
  <text x="10" y="40" font-family="${family}" font-size="24">Bonjour</text>
</svg>`;

// Whatever this environment has, by name: WASI has no system fonts at all, so
// nothing here can assume a family is installed.
const { db, family } = fontsOrSkip(binding, 'font resolution');
const opts = { fonts: db, fontFamily: family };

// 1. a family nobody has is reported, and the text still renders with a fallback
const a = parse(text('Totally Missing Sans'), opts);
assert.deepEqual(a.pendingFonts, ['Totally Missing Sans']);
assert.ok(renderRaw(a.tree).data.length > 0, 'rendered anyway');

// 2. an installed family is not reported
assert.deepEqual(parse(text(family), opts).pendingFonts, []);

// 3. generic families never count as missing
assert.deepEqual(parse(text('sans-serif'), opts).pendingFonts, []);

// 4. deduped, and the CSS fallback list is reported family by family
assert.deepEqual(parse(text('Ghost One, Ghost Two, Ghost One, monospace'), opts).pendingFonts, ['Ghost One', 'Ghost Two']);

// 5. an empty fontdb.Database means every named family is missing
const empty = new fontdb.Database();
const e = parse(text(family), { fonts: empty, fontFamily: family });
assert.deepEqual(e.pendingFonts, [family]);

// 6. the fix loop: with the font, the list goes empty
const f = parse(text(family), opts);
assert.deepEqual(f.pendingFonts, []);
assert.notDeepEqual([...renderRaw(e.tree).data], [...renderRaw(f.tree).data],
  'output actually differs with and without the font');

// 7. images and fonts are reported independently
const g = parse(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="60">
  <image xlink:href="logo" width="40" height="40"/>
  <text x="10" y="55" font-family="Ghost" font-size="12">x</text>
</svg>`, opts);
assert.deepEqual(g.pendingImages, ['logo']);
assert.deepEqual(g.pendingFonts, ['Ghost']);

// 8. a loaded face reports the family name you need for `fontFamily`: the
// first of `families`, `[name, language]` pairs (Q38). The path is
// discovered, not hardcoded: this ran on Fedora only, and every other CI
// host would have failed on it.
const file = testFontFile();
if (file) {
  const loaded = new fontdb.Database();
  loaded.loadFontData(readFileSync(file));
  const [only] = loaded.faces();
  const [[name, language]] = only.families;
  assert.equal(typeof name, 'string');
  assert.equal(language, 'englishUnitedStates', 'English (US) first');
  assert.equal(typeof only.postScriptName, 'string');
  // fontdb resolves by family, and the PostScript name is a different string:
  // true of DejaVu (`DejaVuSans`) and of Arial (`ArialMT`).
  assert.notEqual(only.postScriptName, name);
  const doc = parse(text(name), { fonts: loaded, fontFamily: name });
  assert.deepEqual(doc.pendingFonts, []);
  let opaque = 0;
  const px = renderRaw(doc.tree).data;
  for (let i = 3; i < px.length; i += 4) if (px[i] > 128) opaque++;
  assert.ok(opaque > 100, `text actually rendered (${opaque} opaque pixels)`);
} else {
  skip('loading a font from a file', 'no font file found on this host');
}

// Which face drew a glyph (`faceOf`, `glyph.font`), which families a span
// asked for (`font.families`), and fontdb's `query`, `faceData`,
// `familyName`, `faceSource` and face `ID`s have no form yet: #181, #191.

// The database is shared with the parses that used it and copied on the
// first write, so changing it afterwards leaves a parsed document alone.
if (file) {
  const before = db.len();
  const { tree } = parse(text(family), opts);
  const copy = tree.fontdb();
  copy.loadFontData(readFileSync(file));
  assert.equal(copy.len(), before + 1, 'the copy changed');
  assert.equal(db.len(), before, 'the original did not');
  assert.equal(tree.fontdb().len(), before, 'nor the document');
}
