// Font resolution: same two-pass shape as images (FontResolver is Send + Sync).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('./index.js');

const text = (family) => `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60">
  <text x="10" y="40" font-family="${family}" font-size="24">Bonjour</text>
</svg>`;

// 1. a family nobody has is reported, and the text still renders with a fallback
const a = new Resvg(text('Totally Missing Sans'), { fontFamily: 'DejaVu Sans' });
assert.deepEqual(a.pendingFonts(), ['Totally Missing Sans']);
assert.ok(a.renderPng().length > 0, 'rendered anyway');

// 2. an installed family is not reported
const b = new Resvg(text('DejaVu Sans'), { fontFamily: 'DejaVu Sans' });
assert.deepEqual(b.pendingFonts(), []);

// 3. generic families never count as missing
const c = new Resvg(text('sans-serif'), { fontFamily: 'DejaVu Sans' });
assert.deepEqual(c.pendingFonts(), []);

// 4. deduped, and the CSS fallback list is reported family by family
const d = new Resvg(text('Ghost One, Ghost Two, Ghost One, monospace'), { fontFamily: 'DejaVu Sans' });
assert.deepEqual(d.pendingFonts(), ['Ghost One', 'Ghost Two']);

// 5. an empty FontDatabase means every named family is missing
const empty = new FontDatabase();
const e = new Resvg(text('DejaVu Sans'), { fontFamily: 'DejaVu Sans' }, empty);
assert.deepEqual(e.pendingFonts(), ['DejaVu Sans']);

// 6. the fix loop: load the font, re-parse, list goes empty
const db = new FontDatabase();
db.loadSystemFonts();
const f = new Resvg(text('DejaVu Sans'), { fontFamily: 'DejaVu Sans' }, db);
assert.deepEqual(f.pendingFonts(), []);
assert.notDeepEqual(
  [...new Resvg(text('DejaVu Sans'), { fontFamily: 'DejaVu Sans' }, empty).renderRaw().data],
  [...f.renderRaw().data],
  'output actually differs with and without the font',
);

// 7. images and fonts are reported independently
const g = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="60">
  <image xlink:href="logo" width="40" height="40"/>
  <text x="10" y="55" font-family="Ghost" font-size="12">x</text>
</svg>`, { fontFamily: 'DejaVu Sans' });
assert.deepEqual(g.pendingImages(), ['logo']);
assert.deepEqual(g.pendingFonts(), ['Ghost']);

// 8. a loaded face reports the family name you need for `fontFamily`
{
  const loaded = new FontDatabase();
  loaded.loadFontData(readFileSync('/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf'));
  const [only] = loaded.faces();
  assert.deepEqual(only.families, ['DejaVu Sans']);
  assert.equal(only.postScriptName, 'DejaVuSans', 'not the same string as the family');
  const doc = new Resvg(text(only.families[0]), { fontFamily: only.families[0] }, loaded);
  assert.deepEqual(doc.pendingFonts(), []);
  let opaque = 0;
  const px = doc.renderRaw().data;
  for (let i = 3; i < px.length; i += 4) if (px[i] > 128) opaque++;
  assert.ok(opaque > 100, `text actually rendered (${opaque} opaque pixels)`);
}

console.log('ok — font resolution: all checks passed');
