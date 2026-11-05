// The four examples (`npm run test:examples`), templated as demo/render.mjs
// templates them, then rendered twice: by the flat `Resvg`, and by the
// helpers. Same bytes. Real documents: text, images, QR codes, partials.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Liquid } from 'liquidjs';
import binding from '../../index.js';
import { measureText, renderPng, renderPngAsync } from '../../helpers.mjs';
import { registerSvgFilters } from '../../demo/svg-filters.mjs';

const demo = fileURLToPath(new URL('../../demo/', import.meta.url));

// The demo's fonts, loaded alike in both databases.
const flatFonts = new binding.FontDatabase();
const db = new binding.fontdb.Database();
const files = [join(demo, 'DejaVuSans.ttf'),
  ...readdirSync(join(demo, 'fonts')).filter((f) => /\.(ttf|otf)$/i.test(f)).map((f) => join(demo, 'fonts', f))]
  .filter(existsSync);
for (const file of files) {
  const data = readFileSync(file);
  flatFonts.loadFontData(data);
  db.loadFontData(data);
}
const family = flatFonts.faces()[0].families[0];
for (const set of ['setSansSerifFamily', 'setSerifFamily', 'setMonospaceFamily',
                   'setCursiveFamily', 'setFantasyFamily']) {
  flatFonts[set](family);
  db[set](family);
}

for (const [name, lang] of [['roster'], ['badge-sheet'], ['photo-card'], ['sheet', 'fr']]) {
  test(`${name}.svg: the flat Resvg's bytes`, async () => {
    const template = join(demo, 'examples', `${name}.svg`);
    const options = { fontFamily: family, languages: lang ? [lang] : undefined };
    const liquid = new Liquid({ root: dirname(template), extname: '', relativeReference: false, outputEscape: 'escape' });
    registerSvgFilters(liquid, { measure: (text, o = {}) => measureText(text, { ...options, ...o, fonts: db }) });
    const vars = template.replace(/\.svg$/, '.json');
    const svg = liquid.parseAndRenderSync(readFileSync(template, 'utf8'),
      existsSync(vars) ? JSON.parse(readFileSync(vars, 'utf8')) : {});

    const want = new binding.Resvg(svg, options, flatFonts).renderPng({ scale: 2 });
    assert.ok(renderPng(svg, { ...options, fonts: db, scale: 2 }).equals(want), 'renderPng');
    assert.ok((await renderPngAsync(svg, { ...options, fonts: db, scale: 2 })).equals(want), 'renderPngAsync');
  });
}
