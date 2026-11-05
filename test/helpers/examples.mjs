// The four examples (`npm run test:examples`), templated as demo/render.mjs
// templates them, then rendered twice: through the raw API, as Rust reads, and
// by the helpers. Same bytes. Real documents: text, images, QR codes,
// partials. (Until 0.10 the reference was the flat `Resvg`.)
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

const { usvg, guard, resvg, tiny_skia } = binding;

// The demo's fonts, in the one database both renders use.
const db = new binding.fontdb.Database();
const files = [join(demo, 'DejaVuSans.ttf'),
  ...readdirSync(join(demo, 'fonts')).filter((f) => /\.(ttf|otf)$/i.test(f)).map((f) => join(demo, 'fonts', f))]
  .filter(existsSync);
for (const file of files) db.loadFontData(readFileSync(file));
const family = db.faces()[0].families[0][0];
for (const set of ['setSansSerifFamily', 'setSerifFamily', 'setMonospaceFamily',
                   'setCursiveFamily', 'setFantasyFamily']) {
  db[set](family);
}

/** `svg` at 2x through usvg, tiny-skia and resvg, with the guard the helpers use. */
function rawPng(svg, options) {
  const tree = usvg.Tree.fromStr(svg, {
    ...options, fontdb: db, imageHrefResolver: new guard.ImageHrefResolver(), fontResolver: new guard.FontResolver(),
  });
  const size = tree.size();
  const pixmap = new tiny_skia.Pixmap(Math.ceil(size.width() * 2), Math.ceil(size.height() * 2));
  resvg.render(tree, tiny_skia.Transform.fromScale(2, 2), pixmap);
  return pixmap.encodePng();
}

for (const [name, lang] of [['roster'], ['badge-sheet'], ['photo-card'], ['sheet', 'fr']]) {
  test(`${name}.svg: the raw API's bytes`, async () => {
    const template = join(demo, 'examples', `${name}.svg`);
    const options = { fontFamily: family, languages: lang ? [lang] : undefined };
    const liquid = new Liquid({ root: dirname(template), extname: '', relativeReference: false, outputEscape: 'escape' });
    registerSvgFilters(liquid, { measure: (text, o = {}) => measureText(text, { ...options, ...o, fonts: db }) });
    const vars = template.replace(/\.svg$/, '.json');
    const svg = liquid.parseAndRenderSync(readFileSync(template, 'utf8'),
      existsSync(vars) ? JSON.parse(readFileSync(vars, 'utf8')) : {});

    const want = rawPng(svg, options);
    assert.ok(renderPng(svg, { ...options, fonts: db, scale: 2 }).equals(want), 'renderPng');
    assert.ok((await renderPngAsync(svg, { ...options, fonts: db, scale: 2 })).equals(want), 'renderPngAsync');
  });
}
