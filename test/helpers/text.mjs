// `measureText` and `fitTextWidths` in the helper set (step 11 of
// docs/migration-0.10.md), and the binding-free module beside it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import binding from '../../index.js';
import * as node from '../../helpers.mjs';
import * as browser from '../../helpers.browser.mjs';
import { fontsOrSkip } from '../support.mjs';

const { usvg, tiny_skia } = binding;
const { db, family } = fontsOrSkip(binding, 'helpers: measureText and fitTextWidths');

test('measureText: the width the text draws at, guarded', () => {
  const width = node.measureText('Hello, world', { fonts: db, family, fontSize: 20 });
  // The same text parsed by hand, measured the same way.
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg"><text id="t" font-family="${family}" font-size="20">Hello, world</text></svg>`,
    { fontdb: db });
  assert.equal(width, node.nodeExtent(tree.nodeById('t')).width);
  assert.ok(width > 50);
  // Twice the size, twice the width, give or take hinting.
  const double = node.measureText('Hello, world', { fonts: db, family, fontSize: 40 });
  assert.ok(Math.abs(double / width - 2) < 0.05, `${double} / ${width}`);
  assert.equal(node.measureText('', { fonts: db, family }), 0, 'nothing to draw');
  // fontFamily stands in for family; markup is text, not markup.
  assert.equal(node.measureText('a<b>&c', { fonts: db, fontFamily: family }),
    node.measureText('a<b>&c', { fonts: db, family }));
});

test('fitTextWidths: parse options or a render, one answer', () => {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40">
    <text x="4" y="30" font-size="20" data-maxwidth="60">A rather long name</text>
  </svg>`;
  const byOptions = node.fitTextWidths(svg, { fonts: db, fontFamily: family });
  const byRender = browser.fitTextWidths(svg, browser.guardedParse(binding, { fontdb: db, fontFamily: family }));
  assert.deepEqual(byOptions, byRender);
  assert.equal(byOptions.adjustments.length, 1);
  assert.ok(Math.abs(byOptions.adjustments[0].measured - 60) < 0.5, JSON.stringify(byOptions));
  assert.deepEqual(node.fitTextWidths(svg, (s) => usvg.Tree.fromStr(s, { fontdb: db, fontFamily: family })), byRender, 'a render still works');
});

test('fitTextWidths parses through the guard', () => {
  const dir = mkdtempSync(join(tmpdir(), 'resvg-fit-'));
  const file = join(dir, 'secret.png');
  const p = new tiny_skia.Pixmap(4, 4);
  p.fill(tiny_skia.Color.fromRgba8(255, 0, 0, 255));
  writeFileSync(file, p.encodePng());
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="10">
    <image id="i" href="${file}" width="400" height="10" preserveAspectRatio="none" data-maxwidth="50"/></svg>`;
  // The raw parse reads the file and finds an image to fit; the guard does not.
  assert.equal(browser.fitTextWidths(svg, (s) => usvg.Tree.fromStr(s)).adjustments.length, 1);
  assert.deepEqual(node.fitTextWidths(svg).problems, ['#i has no visible extent: nothing to fit']);
});

test('the browser module: the binding-free functions, and createHelpers', () => {
  assert.deepEqual(Object.keys(browser).sort(), ['MAX_PIXELS', 'createHelpers', 'findWidthConstraints',
    'fitTextWidths', 'guardedParse', 'nodeExtent']);
  // The browser module loads no binding: no default export, no bound helper.
  assert.equal(browser.default, undefined);
  assert.equal(browser.renderPng, undefined);
  assert.equal(typeof browser.createHelpers, 'function');
  // Node's fitTextWidths is the set's, which also takes options.
  assert.notEqual(node.fitTextWidths, browser.fitTextWidths);
  assert.equal(node.nodeExtent, browser.nodeExtent);
});
