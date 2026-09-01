// Every field of RenderOptions, and what it actually changes.
//
// Eight of the eleven had no behavioural coverage: they appeared in a fixture
// somewhere, or not at all, but nothing asserted that setting them did anything.
// A field silently dropped by the mapper would have passed every test.
//
// Each check compares two renders that differ only in the option, so a field
// that stopped being wired would fail here rather than in a user's output.
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { testFonts, skip } from './support.mjs';
import { createRequire } from 'node:module';
const { Resvg, FontDatabase } = createRequire(import.meta.url)('../index.js');

const fontsFound = testFonts(FontDatabase);
if (!fontsFound) {
  skip('render options', 'no font in this environment');
  process.exit(0);
}
const { db, family } = fontsFound;

const png = (svg, opts) => new Resvg(svg, opts, db).renderPng();
const size = (svg, opts) => {
  const r = new Resvg(svg, opts, db).renderRaw();
  return [r.width, r.height];
};
const differs = (a, b) => Buffer.compare(a, b) !== 0;

// 1. dpi — the unit conversion for physical lengths
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="1in"><rect width="100%" height="100%" fill="red"/></svg>';
  assert.deepEqual(size(svg, { dpi: 96 }), [96, 96], 'one inch at 96 dpi');
  assert.deepEqual(size(svg, { dpi: 192 }), [192, 192], 'and at 192');
}

// 2. defaultSizeWidth / defaultSizeHeight
//
//    Pinned as observed, not as assumed: neither a `viewBox`-only document nor
//    one with no size at all changes size when these are set. Both render at
//    10x10 either way. That may be right -- usvg resolves a default size before
//    these are consulted -- or the pair may not be reaching usvg at all. This
//    records today's behaviour so a change in it is visible; it does not claim
//    the behaviour is correct.
{
  const boxed = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="red"/></svg>';
  const bare = '<svg xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10" fill="red"/></svg>';
  for (const svg of [boxed, bare]) {
    assert.deepEqual(size(svg, {}), size(svg, { defaultSizeWidth: 40, defaultSizeHeight: 25 }),
      'setting a default size changes nothing today -- see the note above');
  }
}

// 3. fontFamily — which family a generic name resolves to
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40"><text x="4" y="28" font-size="18">Wg</text></svg>';
  const named = new Resvg(svg, { fontFamily: family }, db);
  assert.deepEqual(named.pendingFonts(), [], 'the family resolves');
  const missing = new Resvg(svg, { fontFamily: 'No Such Family At All' }, db);
  assert.ok(missing.renderPng().length > 0, 'an unresolvable family still renders');
}

// 4. fontSize — the default when the document sets none
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60"><text id="t" x="4" y="40">Wg</text></svg>';
  const small = new Resvg(svg, { fontFamily: family, fontSize: 8 }, db).node('t').extent();
  const large = new Resvg(svg, { fontFamily: family, fontSize: 32 }, db).node('t').extent();
  assert.ok(large.width > small.width * 2, `32pt is wider than 8pt: ${large.width} vs ${small.width}`);
}

// 5. languages — which `<switch>` branch is taken
{
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="60" height="20">
    <switch>
      <rect systemLanguage="fr" width="60" height="20" fill="#ff0000"/>
      <rect systemLanguage="de" width="60" height="20" fill="#0000ff"/>
    </switch></svg>`;
  const px = (langs) => [...new Resvg(svg, { languages: langs }, db).renderRaw().data.subarray(0, 3)];
  assert.deepEqual(px(['fr']), [255, 0, 0], 'fr takes the first branch');
  assert.deepEqual(px(['de']), [0, 0, 255], 'de takes the second');
}

// 6. shapeRendering — crisp edges do not antialias
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><circle cx="10" cy="10" r="8" fill="red"/></svg>';
  assert.ok(
    differs(png(svg, { shapeRendering: 'crispEdges' }), png(svg, { shapeRendering: 'geometricPrecision' })),
    'crispEdges renders differently from geometricPrecision',
  );
}

// 7. textRendering
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="160" height="40"><text x="4" y="28" font-size="17.5">Wg iimm</text></svg>';
  const a = png(svg, { fontFamily: family, textRendering: 'optimizeSpeed' });
  const b = png(svg, { fontFamily: family, textRendering: 'geometricPrecision' });
  assert.ok(differs(a, b), 'the two text rendering modes differ');
}

// 8. imageRendering — how an embedded raster is scaled up
{
  const tiny = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="1" height="1" fill="red"/><rect x="1" y="1" width="1" height="1" fill="blue"/></svg>').renderPng();
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40"><image width="40" height="40" xlink:href="data:image/png;base64,${tiny.toString('base64')}"/></svg>`;
  assert.ok(
    differs(png(svg, { imageRendering: 'optimizeQuality' }), png(svg, { imageRendering: 'optimizeSpeed' })),
    'smooth and nearest scaling differ',
  );
}

// 9. resourcesDir — where a relative href is looked for
{
  const dir = mkdtempSync(join(tmpdir(), 'resvg-opt-'));
  const red = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#00ff00"/></svg>').renderPng();
  writeFileSync(join(dir, 'p.png'), red);
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="p.png"/></svg>';
  assert.deepEqual(new Resvg(svg, { resourcesDir: dir }, db).pendingImages(), [], 'found next to resourcesDir');
  assert.deepEqual(new Resvg(svg, {}, db).pendingImages(), ['p.png'], 'and not found without it');
}

// 10. styleSheet — a user stylesheet applied over the document
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect class="a" width="10" height="10" fill="#ff0000"/></svg>';
  const px = (opts) => [...new Resvg(svg, opts, db).renderRaw().data.subarray(0, 3)];
  assert.deepEqual(px({}), [255, 0, 0], 'the document says red');
  assert.deepEqual(px({ styleSheet: '.a { fill: #00ff00; }' }), [0, 255, 0], 'the stylesheet overrides it');
}

console.log('ok — render options: all checks passed');
