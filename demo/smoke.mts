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
});
