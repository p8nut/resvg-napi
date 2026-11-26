// The browser half of the package, run in a browser: browser.js, the WASI
// browser loader and its worker never execute under `npm test` (Node resolves
// index.js), so until this test their first run was the Pages deploy (#249).
//
// It loads the demo page, which imports resvg.js -- browser.js bundled by
// demo/build.mts -- in headless Chromium, served by demo/serve.mts with the
// cross-origin isolation headers the threads build needs. The page renders its
// first example on load; the test reads a pixel of that PNG back.
//
// Needs the wasm build and the demo bundles (`npm run demo:build`, or in CI the
// wasm artifact and `node demo/build.mts`), and a Chromium for Playwright
// (`npx playwright install chromium`). Not part of `npm test`, so the suite
// needs no browser: `npm run test:browser`.
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';

const serve = fileURLToPath(new URL('./serve.mts', import.meta.url));

// The server's first line names its address (PORT=0: any free port).
async function startServer() {
  const child = spawn(process.execPath, [serve], {
    env: { ...process.env, PORT: '0' },
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  child.stdout.setEncoding('utf8');
  let out = '';
  for await (const chunk of child.stdout) {
    out += chunk;
    const url = out.match(/http:\/\/\S+/)?.[0];
    if (url) return { child, url };
  }
  throw new Error(`demo/serve.mts exited before listening: ${out}`);
}

test('the demo renders through the browser entry', { timeout: 60_000 }, async (t) => {
  const { child, url } = await startServer();
  t.after(async () => {
    child.kill();
    if (child.exitCode === null) await once(child, 'exit');
  });
  const browser = await chromium.launch();
  t.after(() => browser.close());
  const page = await browser.newPage();

  // Anything the page reports as wrong fails the test, the worker's errors
  // included (they reach the page's console), and so does a request the server
  // could not answer: a loader that asks for a file under the wrong name.
  const problems: string[] = [];
  page.on('console', (m) => { if (m.type() === 'error') problems.push(`console: ${m.text()}`); });
  page.on('pageerror', (e) => problems.push(`page error: ${e.message}`));
  page.on('crash', () => problems.push('page crashed'));
  page.on('requestfailed', (r) => problems.push(`request failed: ${r.url()} ${r.failure()?.errorText}`));
  page.on('response', (r) => { if (r.status() >= 400) problems.push(`HTTP ${r.status()}: ${r.url()}`); });

  await page.goto(url);
  // index.html shows the PNG once it rendered, or puts the error in the status
  // line; either ends the wait.
  await page.waitForFunction(() =>
    !document.getElementById('out')?.hidden || document.getElementById('status')?.classList.contains('err'));
  const status = await page.locator('#status').textContent();
  assert.deepEqual(problems, [], `status: ${status}`);
  assert.equal(await page.locator('#status.err').count(), 0, `status: ${status}`);
  assert.equal(await page.evaluate(() => globalThis.crossOriginIsolated), true);

  // The first example, `shapes`, at the page's default scale of 2: a 480x320
  // PNG with a #0091b4 rounded square from (20, 20) to (220, 220).
  const { width, height, pixel } = await page.evaluate(async () => {
    const img = document.getElementById('out') as HTMLImageElement;
    await img.decode();
    const canvas = new OffscreenCanvas(img.naturalWidth, img.naturalHeight);
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('no 2d context');
    ctx.drawImage(img, 0, 0);
    return { width: img.naturalWidth, height: img.naturalHeight, pixel: [...ctx.getImageData(120, 120, 1, 1).data] };
  });
  assert.deepEqual({ width, height }, { width: 480, height: 320 });
  assert.deepEqual(pixel, [0x00, 0x91, 0xb4, 0xff]);

  // The render is async and a change aborts the one before it (#269). A slow
  // document starts rendering (the scale field renders at once, no debounce),
  // then a plain one replaces it while the slow render runs on a worker
  // thread. The plain one is what ends up on screen, the slow one, finishing
  // later, never replaces it, and the main thread kept answering timers
  // throughout: a sync render would hold it for the whole slow render.
  const superseded = await page.evaluate(async () => {
    const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
    const src = $<HTMLTextAreaElement>('src'), scale = $<HTMLInputElement>('scale'), img = $<HTMLImageElement>('out');
    const circles = Array.from({ length: 300 }, (_, i) =>
      `<circle cx="${(i * 37) % 800}" cy="${(i * 91) % 800}" r="30" fill="#0091b4" filter="url(#b)"/>`).join('');
    const slow = `<svg xmlns="http://www.w3.org/2000/svg" width="800" height="800">`
      + `<defs><filter id="b"><feGaussianBlur stdDeviation="8"/></filter></defs>${circles}</svg>`;
    const plain = '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#c8005e"/></svg>';
    const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

    let swaps = 0, gap = 0, last = performance.now(), ticking = true;
    new MutationObserver(() => { swaps += 1; }).observe(img, { attributes: true, attributeFilter: ['src'] });
    const tick = () => {
      const now = performance.now();
      gap = Math.max(gap, now - last);
      last = now;
      if (ticking) setTimeout(tick, 10);
    };
    setTimeout(tick, 10);

    src.value = slow;
    scale.dispatchEvent(new Event('input'));
    await wait(150);                       // the slow render is under way
    src.value = plain;
    scale.dispatchEvent(new Event('input'));
    // Long enough for the slow render (about 2 s here) to come back and be dropped.
    await wait(5000);
    ticking = false;
    await img.decode();
    const ctx = new OffscreenCanvas(img.naturalWidth, img.naturalHeight).getContext('2d');
    if (!ctx) throw new Error('no 2d context');
    ctx.drawImage(img, 0, 0);
    return { swaps, gap, width: img.naturalWidth, pixel: [...ctx.getImageData(5, 5, 1, 1).data],
             err: $('status').classList.contains('err') };
  });
  assert.deepEqual(problems, []);
  assert.equal(superseded.err, false);
  assert.equal(superseded.width, 20, 'the last source, at scale 2');
  assert.deepEqual(superseded.pixel, [0xc8, 0x00, 0x5e, 0xff]);
  assert.equal(superseded.swaps, 1, 'the superseded render never reached the image');
  assert.ok(superseded.gap < 500, `the main thread stalled for ${superseded.gap.toFixed(0)} ms`);
});
