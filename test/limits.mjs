// What an untrusted document can make the process allocate. Each of these
// used to end in a failed allocation, which aborts the process: no try/catch,
// no rejected promise.
import assert from 'node:assert/strict';
import binding from '../index.js';
import { parse, parseAsync, renderPng } from '../helpers.mjs';
import { Readable } from 'node:stream';
import { buffer } from 'node:stream/consumers';
import zlib from 'node:zlib';

const MiB = 1024 * 1024;

// gzip of `n` MiB of spaces, streamed so the test itself never holds them.
const gzipOf = (n, head = '', tail = '') => buffer(Readable.from((function* () {
  yield Buffer.from(head);
  const chunk = Buffer.alloc(MiB, 32);
  for (let i = 0; i < n; i++) yield chunk;
  yield Buffer.from(tail);
})()).pipe(zlib.createGzip({ level: 9 })));

const svg = (body) => `<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">${body}</svg>`;

// 1. SVGZ is not supported: usvg inflates it with no cap (linebender/resvg#1138),
//    so any gzip data is refused before it gets there, whatever it expands to:
//    by the helpers with advice, by the raw API (built without usvg's `svgz`)
//    with usvg's own error.
const bomb = await gzipOf(300, '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><!--', '--></svg>');
assert.ok(bomb.length < 1024 * 1024, `the bomb is small: ${bomb.length} bytes`);
const nested = zlib.gzipSync(bomb);
const red = svg('<rect width="10" height="10" fill="red"/>');
const refused = { code: 'SvgzFeatureNotEnabled', message: /SVGZ is not supported: inflate it first/ };
for (const [what, data] of [['bomb', bomb], ['gzip inside gzip', nested], ['small svgz', zlib.gzipSync(red)]]) {
  assert.throws(() => parse(data, { fonts: [] }), refused, what);
  await assert.rejects(parseAsync(data, { fonts: [] }), refused, `${what}, async`);
  assert.throws(() => binding.usvg.Tree.fromData(data), { code: 'SvgzFeatureNotEnabled' }, `${what}, raw`);
}
// What the message advises works, with a cap of the caller's choosing.
const inflated = zlib.gunzipSync(zlib.gzipSync(red), { maxOutputLength: 1 << 20 });
assert.equal(parse(inflated, { fonts: [] }).tree.size().width(), 10, 'inflated by the caller');

// As an image (supplied, from a file, as a data URI) and the raster cap of
// 2^28 px: the guard's, test/api/guard.mjs.

// 3. The canvas cap defaults to 2^26 px: 8192x8192 is the largest square.
assert.throws(
  () => renderPng('<svg xmlns="http://www.w3.org/2000/svg" width="8193" height="8192"/>', { fonts: [] }),
  /8193x8192 exceeds maxPixels \(67108864\)/,
  'default maxPixels',
);
