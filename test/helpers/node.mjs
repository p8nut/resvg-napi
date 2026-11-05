// `renderNodePng` and its async twin (step 11 of docs/migration-0.10.md; the
// background and sizing #191 asked back): one element in a PNG sized to its
// extent, which is the document rendered with `crop` at that extent when the
// element is all the document draws. The flat `renderNodePng` went with step
// 7b; this compares against the flat `Resvg.renderPng({ crop })`, which stays
// until step 13.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import binding from '../../index.js';
import { nodeExtent, parse, renderNodePng, renderNodePngAsync, renderPng } from '../../helpers.mjs';

const { Resvg } = binding;

const wrap = (body) => `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">${body}</svg>`;

// Each draws one element, `n`, and nothing else.
const ALONE = {
  // A path's extent is its stroke box, and it is offset by its fill box.
  path: wrap('<rect id="n" x="60" y="30" width="60" height="40" fill="tomato" stroke="black" stroke-width="8"/>'),
  // A group's are both its layer box, opacity and filter included.
  group: wrap(`<filter id="f"><feGaussianBlur stdDeviation="4"/></filter>
    <g id="n" opacity="0.8"><rect x="60" y="30" width="60" height="40" fill="tomato"/>
    <circle cx="200" cy="100" r="30" fill="teal" filter="url(#f)"/></g>`),
  // A group with a transform of its own: `renderNode` applies it itself. The
  // flat renderNodePng applied it twice (absTransform), drawing it off its box.
  translated: wrap('<g id="n" transform="translate(10 5)"><path d="M5 150 L80 190 L20 195 Z" fill="purple"/></g>'),
  skewed: wrap('<g id="n" transform="matrix(1 0.2 0.1 1 0 0)"><rect x="150" y="140" width="30" height="20" fill="gold"/></g>'),
  nested: wrap('<g transform="translate(7 3)"><g id="n" transform="scale(1.5)"><circle cx="40" cy="40" r="20" fill="navy"/></g></g>'),
};

const PARAMS = [undefined, { scale: 2 }, { scale: 0.33 }, { width: 101 }, { height: 57 },
  { background: 'teal' }, { background: 'rgba(0, 0, 255, .3)', scale: 1.7 }];

for (const [what, svg] of Object.entries(ALONE)) {
  test(`renderNodePng: the document cropped to the node (${what})`, async () => {
    const flat = new Resvg(svg);
    const { tree } = parse(svg, { fonts: [] });
    const node = tree.nodeById('n');
    const crop = nodeExtent(node);
    for (const params of PARAMS) {
      const want = flat.renderPng({ ...params, crop });
      const label = `${what} ${JSON.stringify(params)}`;
      assert.ok(renderNodePng(node, params).equals(want), label);
      assert.ok((await renderNodePngAsync(node, params)).equals(want), `async ${label}`);
      assert.ok(renderPng(tree, { ...params, crop }).equals(want), `helpers crop ${label}`);
    }
  });
}

test('renderNodePng refuses what has nothing to draw, and crop', async () => {
  const svg = wrap(`<rect id="r" width="10" height="10"/><g id="empty"/>
    <path id="flat" d="M0 0 L10 0" fill="red"/>`);
  const { tree } = parse(svg, { fonts: [] });
  assert.throws(() => renderNodePng(tree.nodeById('r'), { crop: { x: 0, y: 0, width: 5, height: 5 } }),
    { code: 'InvalidArg', message: 'crop is not supported when rendering a single node' });
  // A stroke-less horizontal line: no extent.
  assert.throws(() => renderNodePng(tree.nodeById('flat')), /element has no visible size/);
  assert.throws(() => renderNodePng(tree.nodeById('r'), { maxPixels: 10 }), /10x10 exceeds maxPixels \(10\)/);
  await assert.rejects(renderNodePngAsync(tree.nodeById('r'), { background: 'nope' }), /invalid background: nope/);
  const ac = new AbortController();
  ac.abort();
  await assert.rejects(renderNodePngAsync(tree.nodeById('r'), { signal: ac.signal }));
});
