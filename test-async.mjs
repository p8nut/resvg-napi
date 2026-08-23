// Async rendering: libuv threadpool via napi Task, plus AbortSignal.
// Run with UV_THREADPOOL_SIZE=1 so the abort case is deterministic.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg, renderAsync } = createRequire(import.meta.url)('./index.js');

const isPng = (b) => b.subarray(0, 4).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47]));

// a deliberately slow document: blurred circles
const heavy = `<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">
  <filter id="b"><feGaussianBlur stdDeviation="6"/></filter>
  <g filter="url(#b)">${Array.from({ length: 600 }, (_, i) =>
    `<circle cx="${(i * 37) % 400}" cy="${(i * 53) % 400}" r="14" fill="hsl(${i % 360} 80% 50%)"/>`).join('')}</g>
</svg>`;
const small = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="teal"/></svg>';

// 1. one-shot: parse + render + encode off-thread
assert.ok(isPng(await renderAsync(small)), 'renderAsync returns a PNG');
assert.equal((await renderAsync(small, { dpi: 96 }, { width: 80 })).length > 0, true);

// 2. parseAsync resolves to a real Resvg, reports still work
const doc = await Resvg.parseAsync(small);
assert.equal(doc.width, 40);
assert.deepEqual(doc.pendingImages(), []);
assert.ok(isPng(await doc.renderPngAsync()));

const raw = await doc.renderRawAsync({ scale: 2 });
assert.deepEqual([raw.width, raw.height, raw.data.length], [80, 40, 80 * 40 * 4]);

// 3. the point of all this: the event loop keeps turning during a render
const ticks = (fn) => new Promise(async (done) => {
  let n = 0;
  const t = setInterval(() => n++, 1);
  await fn();
  clearInterval(t);
  done(n);
});
const asyncTicks = await ticks(() => new Resvg(heavy).renderPngAsync({ width: 1600 }));
const syncTicks = await ticks(async () => new Resvg(heavy).renderPng({ width: 1600 }));
console.log(`  timer ticks during render — async: ${asyncTicks}, sync: ${syncTicks}`);
assert.ok(asyncTicks > 0, 'event loop alive during async render');
assert.equal(syncTicks, 0, 'sync render blocks the loop');

// 4. AbortSignal cancels a task still sitting in the queue
const blocker = new Resvg(heavy).renderPngAsync({ width: 2000 }); // occupies the single worker
const ctrl = new AbortController();
const aborted = new Resvg(small).renderPngAsync(null, ctrl.signal);
ctrl.abort();
await assert.rejects(() => aborted, (e) => {
  console.log(`  abort rejected with: ${e.code ?? e.name}: ${e.message}`);
  return true;
});
assert.ok(isPng(await blocker), 'the running task was untouched');

// 5. parallel renders share the pool without stepping on each other
const shots = await Promise.all([1, 2, 3, 4].map((n) => renderAsync(small, null, { width: n * 40 })));
assert.deepEqual(shots.map((b) => isPng(b)), [true, true, true, true]);

console.log('ok — async rendering: all checks passed');
