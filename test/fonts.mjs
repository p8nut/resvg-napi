// Font resolution: same two-pass shape as images (FontResolver is Send + Sync).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { testFonts, testFontFile, skip } from './support.mjs';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('../index.js');

const text = (family) => `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
  <text x="10" y="40" font-family="${family}" font-size="24">Bonjour</text>
</svg>`;

// Whatever this environment has, by name: WASI has no system fonts at all, so
// nothing here can assume a family is installed.
const fonts = testFonts(FontDatabase);
if (!fonts) {
  skip('font resolution', 'no font in this environment');
  process.exit(0);
}
const { db, family } = fonts;
const opts = { fontFamily: family };

// 1. a family nobody has is reported, and the text still renders with a fallback
const a = new Resvg(text('Totally Missing Sans'), opts, db);
assert.deepEqual(a.pendingFonts(), ['Totally Missing Sans']);
assert.ok(a.renderPng().length > 0, 'rendered anyway');

// 2. an installed family is not reported
const b = new Resvg(text(family), opts, db);
assert.deepEqual(b.pendingFonts(), []);

// 3. generic families never count as missing
const c = new Resvg(text('sans-serif'), opts, db);
assert.deepEqual(c.pendingFonts(), []);

// 4. deduped, and the CSS fallback list is reported family by family
const d = new Resvg(text('Ghost One, Ghost Two, Ghost One, monospace'), opts, db);
assert.deepEqual(d.pendingFonts(), ['Ghost One', 'Ghost Two']);

// 5. an empty FontDatabase means every named family is missing
const empty = new FontDatabase();
const e = new Resvg(text(family), opts, empty);
assert.deepEqual(e.pendingFonts(), [family]);

// 6. the fix loop: with the font, the list goes empty
const f = new Resvg(text(family), opts, db);
assert.deepEqual(f.pendingFonts(), []);
assert.notDeepEqual(
  [...new Resvg(text(family), opts, empty).renderRaw().data],
  [...f.renderRaw().data],
  'output actually differs with and without the font',
);

// 7. images and fonts are reported independently
const g = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="60">
  <image xlink:href="logo" width="40" height="40"/>
  <text x="10" y="55" font-family="Ghost" font-size="12">x</text>
</svg>`, opts, db);
assert.deepEqual(g.pendingImages(), ['logo']);
assert.deepEqual(g.pendingFonts(), ['Ghost']);

// 8. a loaded face reports the family name you need for `fontFamily`
// The path is discovered, not hardcoded: this ran on Fedora only, and every
// other CI host would have failed on it.
const file = testFontFile();
if (file) {
  const loaded = new FontDatabase();
  loaded.loadFontData(readFileSync(file));
  const [only] = loaded.faces();
  assert.ok(only.families.length > 0 && typeof only.families[0] === 'string');
  assert.equal(typeof only.postScriptName, 'string');
  // fontdb resolves by family, and the PostScript name is a different string:
  // true of DejaVu (`DejaVuSans`) and of Arial (`ArialMT`).
  assert.notEqual(only.postScriptName, only.families[0]);
  const doc = new Resvg(text(only.families[0]), { fontFamily: only.families[0] }, loaded);
  assert.deepEqual(doc.pendingFonts(), []);
  let opaque = 0;
  const px = doc.renderRaw().data;
  for (let i = 3; i < px.length; i += 4) if (px[i] > 128) opaque++;
  assert.ok(opaque > 100, `text actually rendered (${opaque} opaque pixels)`);
} else {
  skip('loading a font from a file', 'no font file found on this host');
}

// 9. `pendingFonts()` names what was missing. This names what drew the text
// instead: a fallback that renders is the failure nobody sees, and the report
// alone cannot say which glyph got which face.
const withId = (family) => `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
  <text id="t" x="10" y="40" font-family="${family}" font-size="24">Bonjour</text>
</svg>`;

const missing = new Resvg(withId('Totally Missing Sans'), opts, db);
const glyphs = missing.node('t').text().layouted[0].positionedGlyphs;
assert.ok(glyphs.length > 0, 'the fallback laid glyphs out');
const used = missing.faceOf(glyphs[0]);
assert.ok(used, 'the glyph resolves to the face that drew it');
assert.equal(typeof used.postScriptName, 'string');
assert.ok(
  !used.families.includes('Totally Missing Sans'),
  'and that face is not the family the document asked for',
);

// 10. the other half of the comparison, so a caller can tell agreement from
// substitution without knowing what it asked for: the families of the span.
const askedFor = missing.node('t').text().chunks[0].spans[0].font.families;
assert.ok(
  askedFor.includes('Totally Missing Sans'),
  `the span reports the requested family (got ${JSON.stringify(askedFor)})`,
);

// 11. with the family installed, asked-for and used agree -- the same two
// calls, answering "no substitution here".
const present = new Resvg(withId(family), opts, db);
const presentFace = present.faceOf(present.node('t').text().layouted[0].positionedGlyphs[0]);
assert.ok(presentFace.families.includes(family), `used ${presentFace.families} for ${family}`);

// 12. a generic family keeps its CSS spelling rather than becoming a name:
// usvg holds `FontFamily::SansSerif`, and the string has to say so.
const generic = new Resvg(withId('sans-serif'), opts, db);
assert.ok(
  generic.node('t').text().chunks[0].spans[0].font.families.includes('sans-serif'),
  'a generic family reads back as its keyword',
);

// 13. a glyph from a document parsed by another instance is not resolvable
// against this one: the lookup is the document's own database, not a global.
assert.equal(new Resvg(withId(family), opts, new FontDatabase()).faceOf(glyphs[0]), null);

console.log('ok — font resolution: all checks passed');
