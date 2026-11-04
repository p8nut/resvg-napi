// The demo's Liquid filters cut text, and a cut on a UTF-16 index can land inside
// an emoji: half a surrogate pair is not a character, and the SVG gets a broken
// one. No renderer needed, so the width is ten per UTF-16 unit.
import assert from 'node:assert/strict';
import { Liquid } from 'liquidjs';
import { registerSvgFilters } from '../demo/svg-filters.mjs';

const liquid = new Liquid({ outputEscape: 'escape' });
registerSvgFilters(liquid, { measure: (text) => text.length * 10, qrSvg: () => '' });
const loneSurrogate = /[\uD800-\uDFFF]/u;

for (const [template, s, want] of [
  ['{{ s | fit: 60 }}', 'abcd🚀🚀🚀', 'abcd…'],
  // liquidjs's own, fixed in 10.30.1: the floor in package.json is what keeps it.
  ['{{ s | truncate: 5 }}', 'a🚀🚀🚀🚀🚀', 'a🚀...'],
]) {
  const out = liquid.parseAndRenderSync(template, { s });
  assert.ok(!loneSurrogate.test(out), `${template} split a surrogate pair: ${JSON.stringify(out)}`);
  assert.equal(out, want, template);
}
