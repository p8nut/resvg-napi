// Font resolution: same two-pass shape as images (FontResolver is Send + Sync).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { Resvg, FontDatabase } from '../index.js';
import { fontsOrSkip, testFontFile, skip } from './support.mjs';

const text = (family) => `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
  <text x="10" y="40" font-family="${family}" font-size="24">Bonjour</text>
</svg>`;

// Whatever this environment has, by name: WASI has no system fonts at all, so
// nothing here can assume a family is installed.
const { db, family } = fontsOrSkip(FontDatabase, 'font resolution');
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

// 9-13. Which face drew a glyph (`faceOf`, `glyph.font`) and which families a
// span asked for (`font.families`) went with the flat node classes. The
// namespaced `usvg.PositionedGlyph` carries no face and `usvg.Font` no family
// names yet: left for later (docs/migration-0.10.md).

// A face from another database does not name a face of this one. IDs are
// slotmap keys, so the first face of two databases share a key; before the
// check, B.face(faceOfA) answered with B's own face and B.removeFace(faceOfA)
// deleted it.
{
  const file = testFontFile();
  if (file) {
    const b = new FontDatabase();
    b.loadFontData(readFileSync(file));
    const mine = b.faces()[0];
    const foreign = db.faces()[0];
    assert.equal(b.face(mine)?.postScriptName, mine.postScriptName, 'own face found');
    // Only when the system database starts with a different face, which
    // a container holding just the test font does not.
    if (foreign.postScriptName !== mine.postScriptName) {
      assert.equal(b.face(foreign), null, 'foreign face not found');
      b.removeFace(foreign);
      assert.equal(b.len(), 1, 'and not removed in its place');
    }
    assert.equal(new FontDatabase().face(mine), null, 'empty database');
  }
}

// The database is shared with the parses that used it and copied on the
// first write, so changing it afterwards leaves a parsed document alone.
{
  const before = db.len();
  const doc = new Resvg(text(family), opts, db);
  const copy = doc.fontdb();
  copy.removeFace(copy.faces()[0]);
  assert.equal(copy.len(), before - 1, 'the copy changed');
  assert.equal(db.len(), before, 'the original did not');
  assert.equal(doc.fontdb().len(), before, 'nor the document');
}

// familyName: a generic keyword resolves to the family the database maps it
// to, any other name comes back unchanged
{
  const fresh = new FontDatabase();
  for (const generic of ['serif', 'sans-serif', 'cursive', 'fantasy', 'monospace']) {
    const name = fresh.familyName(generic);
    assert.ok(name.length > 0 && name !== generic, `${generic} -> ${name}`);
  }
  assert.equal(fresh.familyName('My Font'), 'My Font');
}

// faceSource: where a face came from, and its index in that file
{
  const file = testFontFile();
  if (file) {
    const fromData = new FontDatabase();
    fromData.loadFontData(readFileSync(file));
    const face = fromData.faces()[0];
    assert.deepEqual({ ...fromData.faceSource(face) }, { kind: 'binary', index: face.index });
    assert.equal(new FontDatabase().faceSource(face), null, 'not a face of an empty database');

    // WASI has no filesystem for fontdb to open
    let fromFile = null;
    try {
      fromFile = new FontDatabase();
      fromFile.loadFontFile(file);
    } catch {
      fromFile = null;
    }
    if (fromFile) {
      const src = fromFile.faceSource(fromFile.faces()[0]);
      assert.ok(['file', 'sharedFile'].includes(src.kind), src.kind);
      assert.equal(src.path, file);
    }
  }
}
