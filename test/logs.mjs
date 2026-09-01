// The log collector: usvg and resvg report recoverable problems through the
// `log` crate, and nothing consumed it before.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg, setLogLevel, takeLogs } = createRequire(import.meta.url)('../index.js');

const messy = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="50" height="50">
  <rect width="10" height="10" fill="notacolour"/>
  <rect width="0" height="10" stroke="red"/>
  <image xlink:href="data:image/png;base64,AAAA" width="5" height="5"/>
</svg>`;

// 1. nothing is collected until a level is set
takeLogs();
new Resvg(messy).renderPng();
assert.deepEqual(takeLogs(), [], 'silent by default');

// 2. warnings arrive, tagged with level and module
setLogLevel('warn');
new Resvg(messy).renderPng();
const logs = takeLogs();
assert.ok(logs.length >= 2, `got ${logs.length} messages`);
assert.ok(logs.every((l) => /^(ERROR|WARN) \S+: /.test(l)), logs.join('\n'));
assert.ok(logs.some((l) => l.includes("Failed to parse fill value: 'notacolour'")));
assert.ok(logs.some((l) => l.includes('usvg::parser::shapes')));

// 3. draining is a drain
assert.deepEqual(takeLogs(), []);

// 4. the level filters
setLogLevel('off');
new Resvg(messy).renderPng();
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
new Resvg(many).renderPng();
const bounded = takeLogs();
assert.ok(bounded.length > 100, `the document did produce warnings, got ${bounded.length}`);
assert.equal(bounded.length, 500, 'capped at exactly 500, not merely under it');
setLogLevel('off');

console.log('ok — log collection: all checks passed');
