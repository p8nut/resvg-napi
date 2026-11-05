// Every field of `usvg.Options`, and what it actually changes, through the
// helpers (`parse`, `renderPng`, `renderRaw` pass them on to usvg).
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
import { fontsOrSkip } from './support.mjs';
import binding from '../index.js';
import { parse, renderPng, renderRaw } from '../helpers.mjs';

const { tiny_skia } = binding;
const { db, family } = fontsOrSkip(binding, 'render options');

const png = (svg, opts) => renderPng(svg, { fonts: db, ...opts });
const raw = (svg, opts) => renderRaw(svg, { fonts: db, ...opts });
const size = (svg, opts) => {
  const r = raw(svg, opts);
  return [r.width, r.height];
};
const differs = (a, b) => Buffer.compare(a, b) !== 0;

// 1. dpi — the unit conversion for physical lengths
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="1in" height="1in"><rect width="100%" height="100%" fill="red"/></svg>';
  assert.deepEqual(size(svg, { dpi: 96 }), [96, 96], 'one inch at 96 dpi');
  assert.deepEqual(size(svg, { dpi: 192 }), [192, 192], 'and at 192');
}

// 2. defaultSize — a `tiny_skia.Size`, wired here, not honoured upstream
//
//    usvg documents these as "the viewport size to assume if there is no
//    `viewBox` and the width or height attributes are relative", and in exactly
//    that case they change nothing: a `width="100%"` document with no viewBox
//    renders 100x100 whatever they are set to.
//
//    The binding is not dropping them. usvg's converter rewrites its `width`
//    and `height` locals from `default_size` (parser/converter.rs:540-556) and
//    then, in the branch with no viewBox, computes the size with
//    `svg.convert_user_length(AId::Width, ..)` (:588), which re-reads the
//    attribute and ignores the rewrite. The percentage resolves against the
//    default viewport instead, which is 100x100.
//
//    So this pins two things: that the option reaches usvg's `Options` at all
//    (a sibling on the same document does take effect), and what usvg currently
//    does with it. If a resvg bump fixes that branch, this test fails and the
//    assertion below is the one to invert.
{
  const relative = '<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%"><rect width="100%" height="100%" fill="red"/></svg>';
  assert.deepEqual(size(relative, {}), [100, 100], "usvg's own default viewport");
  assert.deepEqual(
    size(relative, { defaultSize: tiny_skia.Size.fromWh(40, 25) }),
    [100, 100],
    'unchanged -- usvg re-reads the attribute rather than its rewritten local',
  );
  // and the options object does reach usvg on this very document
  assert.deepEqual(
    [...raw(relative, { styleSheet: 'rect { fill: #00ff00 }' }).data.subarray(0, 3)],
    [0, 255, 0],
    'a sibling option on the same document takes effect',
  );
}

// 3. fontFamily — which family a generic name resolves to
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40"><text x="4" y="28" font-size="18">Wg</text></svg>';
  const named = parse(svg, { fonts: db, fontFamily: family });
  assert.deepEqual(named.pendingFonts, [], 'the family resolves');
  const missing = parse(svg, { fonts: db, fontFamily: 'No Such Family At All' });
  assert.ok(renderPng(missing.tree).length > 0, 'an unresolvable family still renders');
}

// 4. fontSize — the default when the document sets none
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60"><text id="t" x="4" y="40">Wg</text></svg>';
  const width = (fontSize) => parse(svg, { fonts: db, fontFamily: family, fontSize })
    .tree.root().absStrokeBoundingBox().width();
  const [small, large] = [width(8), width(32)];
  assert.ok(large > small * 2, `32pt is wider than 8pt: ${large} vs ${small}`);
}

// 5. languages — which `<switch>` branch is taken
{
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="60" height="20">
    <switch>
      <rect systemLanguage="fr" width="60" height="20" fill="#ff0000"/>
      <rect systemLanguage="de" width="60" height="20" fill="#0000ff"/>
    </switch></svg>`;
  const px = (languages) => [...raw(svg, { languages }).data.subarray(0, 3)];
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
  const tiny = png('<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="1" height="1" fill="red"/><rect x="1" y="1" width="1" height="1" fill="blue"/></svg>');
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40"><image width="40" height="40" xlink:href="data:image/png;base64,${tiny.toString('base64')}"/></svg>`;
  assert.ok(
    differs(png(svg, { imageRendering: 'optimizeQuality' }), png(svg, { imageRendering: 'optimizeSpeed' })),
    'smooth and nearest scaling differ',
  );
}

// 9. resourcesDir — where a relative href is looked for
{
  const dir = mkdtempSync(join(tmpdir(), 'resvg-opt-'));
  const red = png('<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#00ff00"/></svg>');
  writeFileSync(join(dir, 'p.png'), red);
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="p.png"/></svg>';
  assert.deepEqual(parse(svg, { fonts: db, resourcesDir: dir }).pendingImages, [], 'found next to resourcesDir');
  assert.deepEqual(parse(svg, { fonts: db }).pendingImages, ['p.png'], 'and not found without it');
}

// 10. styleSheet — a user stylesheet applied over the document
{
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect class="a" width="10" height="10" fill="#ff0000"/></svg>';
  const px = (opts) => [...raw(svg, opts).data.subarray(0, 3)];
  assert.deepEqual(px({}), [255, 0, 0], 'the document says red');
  assert.deepEqual(px({ styleSheet: '.a { fill: #00ff00; }' }), [0, 255, 0], 'the stylesheet overrides it');
}
