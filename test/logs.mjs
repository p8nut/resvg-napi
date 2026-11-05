// The log collector: usvg and resvg report recoverable problems through the
// `log` crate, and nothing consumed it before. One buffer for the process
// (ADR 0001: per-document logs went with the 0.9 `Resvg`).
import assert from 'node:assert/strict';
import { setLogLevel, takeLogs, usvg, tiny_skia, resvg } from '../index.js';
import { renderPng, renderPngAsync } from '../helpers.mjs';

const messy = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="50" height="50">
  <rect width="10" height="10" fill="notacolour"/>
  <rect width="0" height="10" stroke="red"/>
  <image xlink:href="data:image/png;base64,AAAA" width="5" height="5"/>
</svg>`;
const render = (svg) => renderPng(svg, { fonts: [] });

// 1. nothing is collected until a level is set
takeLogs();
render(messy);
assert.deepEqual(takeLogs(), [], 'silent by default');

// 2. warnings arrive, tagged with level and module
setLogLevel('warn');
render(messy);
const logs = takeLogs();
assert.ok(logs.length >= 2, `got ${logs.length} messages`);
assert.ok(logs.every((l) => /^(ERROR|WARN) \S+: /.test(l)), logs.join('\n'));
assert.ok(logs.some((l) => l.includes("Failed to parse fill value: 'notacolour'")));
assert.ok(logs.some((l) => l.includes('usvg::parser::shapes')));

// 3. draining is a drain
assert.deepEqual(takeLogs(), []);

// 4. the level filters
setLogLevel('off');
render(messy);
assert.deepEqual(takeLogs(), [], 'off collects nothing');

// 5. a bad level is an error, not a silent no-op
assert.throws(() => setLogLevel('louder'), /unknown log level/);

// 6. the buffer is bounded: a pathological document cannot grow it forever
//
//    400 shapes against a 500 cap could not fail -- it asserted `<= 500` on a
//    number that could not exceed it, so an unbounded buffer would have passed.
//    1200 is comfortably past the cap, and the assertion is that it *stopped*.
setLogLevel('warn');
const many = `<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">${
  Array.from({ length: 1200 }, (_, i) => `<rect width="0" height="1" stroke="red" id="r${i}"/>`).join('')
}</svg>`;
render(many);
const bounded = takeLogs();
assert.ok(bounded.length > 100, `the document did produce warnings, got ${bounded.length}`);
assert.equal(bounded.length, 500, 'capped at exactly 500, not merely under it');
// ...and says so: without a marker, 500 lines read as everything there was.
assert.match(bounded.at(-1), /^WARN napi_support::logs: log buffer full .*dropped$/, 'truncation is marked');
assert.ok(!bounded.slice(0, -1).some((l) => /buffer full/.test(l)), 'one marker, last');
takeLogs();

// 7. what a worker thread reports lands in the same buffer: a PNG cut after
//    its header parses (usvg reads only the size) and fails to decode at
//    render time, on the worker thread an async twin runs on.
{
  const png = render('<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>');
  const bad = `<image href="data:image/png;base64,${png.subarray(0, 33).toString('base64')}" width="5" height="5"/><rect width="5" height="5"/>`;
  const broken = `<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">${bad}</svg>`;
  takeLogs();
  await renderPngAsync(broken, { fonts: [] });
  const fromWorker = takeLogs();
  assert.equal(fromWorker.filter((l) => l.includes('Failed to decode a PNG image')).length, 1, fromWorker.join('\n'));

  // A node drawn through resvg.renderNode reports too, and so does one
  // reached through a definition's content.
  const draw = (node) => {
    const pixmap = new tiny_skia.Pixmap(20, 20);
    resvg.renderNode(node, tiny_skia.Transform.identity(), pixmap);
  };
  const sprite = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><g id="icon">${bad}</g></svg>`);
  takeLogs();
  draw(sprite.nodeById('icon'));
  const fromNode = takeLogs();
  assert.equal(fromNode.filter((l) => l.includes('Failed to decode a PNG image')).length, 1, fromNode.join('\n'));
  const patterned = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><defs><pattern id="p" width="10" height="10" patternUnits="userSpaceOnUse"><g>${bad}</g></pattern></defs><rect width="20" height="20" fill="url(#p)"/></svg>`);
  takeLogs();
  draw(patterned.patterns()[0].root().children()[0]);
  const fromDef = takeLogs();
  assert.equal(fromDef.filter((l) => l.includes('Failed to decode a PNG image')).length, 1, fromDef.join('\n'));
  // Its async twin draws on a worker thread, which reports there too.
  takeLogs();
  await resvg.renderNodeAsync(sprite.nodeById('icon'), tiny_skia.Transform.identity(), new tiny_skia.Pixmap(20, 20));
  const fromNodeWorker = takeLogs();
  assert.equal(fromNodeWorker.filter((l) => l.includes('Failed to decode a PNG image')).length, 1, fromNodeWorker.join('\n'));
}
setLogLevel('off');
