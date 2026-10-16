// What an untrusted document can make the process allocate. Each of these
// used to end in a failed allocation, which aborts the process: no try/catch,
// no rejected promise.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { Readable } from 'node:stream';
import { buffer } from 'node:stream/consumers';
import zlib from 'node:zlib';
const { Resvg, setLogLevel, takeLogs } = createRequire(import.meta.url)('../index.js');

const MiB = 1024 * 1024;

// gzip of `n` MiB of spaces, streamed so the test itself never holds them.
const gzipOf = (n, head = '', tail = '') => buffer(Readable.from((function* () {
  yield Buffer.from(head);
  const chunk = Buffer.alloc(MiB, 32);
  for (let i = 0; i < n; i++) yield chunk;
  yield Buffer.from(tail);
})()).pipe(zlib.createGzip({ level: 9 })));

const svg = (body) => `<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">${body}</svg>`;

// 1. SVGZ that inflates past 256 MiB is refused, at the top level...
const bomb = await gzipOf(257, '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><!--', '--></svg>');
assert.ok(bomb.length < 1024 * 1024, `the bomb is small: ${bomb.length} bytes`);
assert.throws(() => new Resvg(bomb), /SVGZ data expands past 268435456 bytes/, 'top-level svgz bomb');
await assert.rejects(Resvg.parseAsync(bomb), /SVGZ data expands past/, 'async too');

// ...while a normal one still parses.
const small = zlib.gzipSync(svg('<rect width="5" height="5"/>'));
assert.equal(new Resvg(small).width, 10, 'svgz under the cap');

// ...and as an image: supplied, or as a data URI, where usvg's own resolver
// would have inflated it. Refused images are logged and drawn as nothing.
setLogLevel('warn');
takeLogs();
const img = (href) => svg(`<image href="${href}" width="10" height="10"/>`);
const supplied = new Resvg(img('bomb.svgz'), null, null, { 'bomb.svgz': bomb });
assert.deepEqual(supplied.pendingImages(), ['bomb.svgz'], 'supplied svgz bomb refused');
new Resvg(img(`data:image/svg+xml;base64,${bomb.toString('base64')}`)).renderPng();
assert.ok(takeLogs().filter((l) => /image refused, gzip data expands past/.test(l)).length >= 2, 'both refusals logged');

// 2. A raster image whose header declares more than 2^28 px is refused before
//    resvg decodes it: an IHDR claiming 65535x65535 with no pixel data.
const crc = (buf) => {
  let c = ~0;
  for (const b of buf) { c ^= b; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
  return ~c >>> 0;
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(65535, 0);
ihdr.writeUInt32BE(65535, 4);
ihdr.set([8, 6, 0, 0, 0], 8);
const chunk = (type, data) => {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, 'latin1');
  data.copy(out, 8);
  out.writeUInt32BE(crc(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
};
const huge = Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IEND', Buffer.alloc(0))]);
const big = new Resvg(img('huge.png'), null, null, { 'huge.png': huge });
assert.deepEqual(big.pendingImages(), ['huge.png'], 'oversized raster refused');
big.renderPng();
new Resvg(img(`data:image/png;base64,${huge.toString('base64')}`)).renderPng();
assert.ok(takeLogs().filter((l) => /image refused, 65535x65535 exceeds 268435456 px/.test(l)).length >= 2, 'logged');
setLogLevel('off');

// 3. The canvas cap defaults to 2^26 px: 8192x8192 is the largest square.
assert.throws(
  () => new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="8193" height="8192"/>').renderPng(),
  /8193x8192 exceeds maxPixels \(67108864\)/,
  'default maxPixels',
);

console.log('ok — limits: all checks passed');
