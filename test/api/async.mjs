// The async twins of the namespaced surface (ADR 0001; ADR 0002, Q23, Q28,
// Q29): `<name>Async(...args, signal?)` for what `GEN.asynchronous` lists.
// The call runs on a worker thread and its promise settles with what the sync
// call returns, or rejects with the `code` and `message` it throws. A
// `tiny_skia.Pixmap` an async render draws on is leased: in use until the
// promise settles, whatever way it settles.
//
// Run with one worker thread, so a call queued behind a slow one is still
// queued when its signal fires: `UV_THREADPOOL_SIZE=1`, which the WASI loader
// reads as well. Set by a re-exec, as in `test/async.mjs`, and so plain
// asserts: the re-exec'd process reports to the terminal, not to the runner.
import assert from 'node:assert/strict';

if (process.env.UV_THREADPOOL_SIZE !== '1') {
  const { spawnSync } = await import('node:child_process');
  const { fileURLToPath } = await import('node:url');
  const { status } = spawnSync(process.execPath, [fileURLToPath(import.meta.url)], {
    stdio: 'inherit',
    env: { ...process.env, UV_THREADPOOL_SIZE: '1' },
  });
  process.exit(status ?? 1);
}

const { readFileSync } = await import('node:fs');
const { fileURLToPath } = await import('node:url');
const { default: binding } = await import('../../index.js');
const { usvg, fontdb, tiny_skia, resvg } = binding;

const FONT_DIR = new URL('../../demo/fonts/', import.meta.url);
const FONT = readFileSync(new URL('IBMPlexSans-Latin.ttf', FONT_DIR));
const FAMILY = 'IBM Plex Sans';

const SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80">
  <defs><filter id="b"><feGaussianBlur stdDeviation="2"/></filter></defs>
  <g id="g" opacity="0.8">
    <rect x="10" y="10" width="40" height="30" fill="tomato" stroke="black" stroke-width="4" filter="url(#b)"/>
    <circle cx="70" cy="40" r="15" fill="teal"/>
  </g>
  <text x="4" y="76" font-family="${FAMILY}" font-size="12">Hello</text>
</svg>`;

// Slow enough to keep the one worker thread busy: blurred circles.
const HEAVY = `<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">
  <filter id="b"><feGaussianBlur stdDeviation="6"/></filter>
  <g filter="url(#b)">${Array.from({ length: 600 }, (_, i) =>
    `<circle cx="${(i * 37) % 400}" cy="${(i * 53) % 400}" r="14" fill="hsl(${i % 360} 80% 50%)"/>`).join('')}</g>
</svg>`;

function coded(code, message) {
  return (e) => {
    assert.ok(e instanceof Error);
    assert.equal(e.code, code);
    if (message instanceof RegExp) assert.match(e.message, message);
    else assert.equal(e.message, message);
    return true;
  };
}

/** What `fn` throws, for comparing an async twin's rejection with it. */
function thrownBy(fn) {
  try {
    fn();
  } catch (e) {
    return e;
  }
  assert.fail('expected a throw');
}

const db = new fontdb.Database();
db.loadFontData(FONT);
const options = { fontdb: db, fontFamily: FAMILY };
const transform = tiny_skia.Transform.identity();

// 1. Each twin gives what its sync half gives.
{
  const tree = usvg.Tree.fromStr(SVG, options);
  const fromStr = await usvg.Tree.fromStrAsync(SVG, options);
  const fromData = await usvg.Tree.fromDataAsync(Buffer.from(SVG), options);
  assert.ok(fromStr instanceof usvg.Tree && fromData instanceof usvg.Tree);
  assert.equal(fromStr.toString(), tree.toString(), 'fromStrAsync');
  assert.equal(fromData.toString(), tree.toString(), 'fromDataAsync');
  assert.equal(await tree.toStringAsync(), tree.toString(), 'toStringAsync');
  assert.equal(await tree.toStringAsync({ indent: 'tabs' }), tree.toString({ indent: 'tabs' }));

  const sync = new tiny_skia.Pixmap(120, 80);
  resvg.render(tree, transform, sync);
  const pixmap = new tiny_skia.Pixmap(120, 80);
  assert.equal(await resvg.renderAsync(fromStr, transform, pixmap), undefined);
  assert.ok(Buffer.compare(pixmap.data(), sync.data()) === 0, 'renderAsync: the same pixels');
  assert.ok(pixmap.data().some((b) => b !== 0), 'and something was drawn');
  const png = await pixmap.encodePngAsync();
  assert.ok(Buffer.compare(png, sync.encodePng()) === 0, 'encodePngAsync');

  // renderNode's twin, against its sync half: the node offset as Rust does.
  const g = tree.nodeById('g');
  const node = new tiny_skia.Pixmap(120, 80);
  assert.equal(resvg.renderNode(g, transform, node), true);
  const nodeAsync = new tiny_skia.Pixmap(120, 80);
  assert.equal(await resvg.renderNodeAsync(g, transform, nodeAsync), true);
  assert.ok(Buffer.compare(nodeAsync.data(), node.data()) === 0, 'renderNodeAsync: the same pixels');
  // `None` (a node of no size: a filled line) is null, as in sync.
  const flat = usvg.Tree.fromStr('<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path id="p" d="M1 1 L 1 5" fill="red"/></svg>');
  const line = flat.nodeById('p');
  assert.equal(resvg.renderNode(line, transform, new tiny_skia.Pixmap(4, 4)), null);
  assert.equal(await resvg.renderNodeAsync(line, transform, new tiny_skia.Pixmap(4, 4)), null);

  // Fonts loaded on a worker thread.
  const dir = new fontdb.Database();
  assert.equal(await dir.loadFontsDirAsync(fileURLToPath(FONT_DIR)), undefined);
  assert.ok(dir.len() > 0, 'loadFontsDirAsync found the repository\'s fonts');
  const sys = new fontdb.Database();
  assert.equal(await sys.loadSystemFontsAsync(), undefined);
  assert.ok(sys.len() >= 0);
}

// 2. An error rejects with the code and message the sync call throws (Q23).
{
  const cases = {
    ParsingFailed: '<svg',
    InvalidSize: '<svg xmlns="http://www.w3.org/2000/svg" width="0" height="10"/>',
  };
  for (const [code, svg] of Object.entries(cases)) {
    assert.ok(Object.values(usvg.ErrorCode).includes(code));
    const sync = thrownBy(() => usvg.Tree.fromStr(svg));
    await assert.rejects(usvg.Tree.fromStrAsync(svg), coded(code, sync.message), `fromStrAsync: ${code}`);
    await assert.rejects(usvg.Tree.fromDataAsync(Buffer.from(svg)), coded(code, sync.message), `fromDataAsync: ${code}`);
  }
  const notUtf8 = new Uint8Array([0x3c, 0xff, 0xfe]);
  await assert.rejects(usvg.Tree.fromDataAsync(notUtf8),
    coded(usvg.ErrorCode.NotAnUtf8Str, thrownBy(() => usvg.Tree.fromData(notUtf8)).message));
  // A twin's own argument errors throw at the call, as napi's conversions do.
  assert.throws(() => resvg.renderAsync({}, transform, new tiny_skia.Pixmap(1, 1)));
  const disposed = usvg.Tree.fromStr(SVG);
  disposed.dispose();
  assert.throws(() => resvg.renderAsync(disposed, transform, new tiny_skia.Pixmap(1, 1)), coded('Disposed', 'this Tree was disposed'));
}

// 3. The pixmap a render draws on is leased (Q28): every use of it throws
//    until the promise settles, then it is back, drawn on.
{
  const tree = usvg.Tree.fromStr(SVG, options);
  const pixmap = new tiny_skia.Pixmap(120, 80);
  const inUse = coded('Leased', 'this Pixmap is in use by an async call');
  const pending = resvg.renderAsync(tree, transform, pixmap);
  assert.throws(() => pixmap.width(), inUse);
  assert.throws(() => pixmap.data(), inUse);
  assert.throws(() => pixmap.encodePng(), inUse);
  assert.throws(() => resvg.render(tree, transform, pixmap), inUse, 'a sync render');
  assert.throws(() => resvg.renderAsync(tree, transform, pixmap), inUse, 'a second async render');
  // dispose() refuses while the value is in use: nothing changed.
  assert.throws(() => pixmap.dispose(), inUse);
  assert.equal(pixmap.disposed, false);
  // The tree is an `Arc`: cloned for the call, not leased.
  assert.ok(tree.size().width() > 0);
  await pending;
  assert.equal(pixmap.width(), 120);
  assert.ok(pixmap.data().some((b) => b !== 0));

  // Encoding reads the pixmap on the worker thread: leased as well.
  const encoding = pixmap.encodePngAsync();
  assert.throws(() => pixmap.width(), inUse);
  assert.ok((await encoding).length > 0);
  pixmap.dispose();
  assert.throws(() => pixmap.width(), coded('Disposed', 'this Pixmap was disposed'));

  // A node keeps its tree: disposing the tree while a node render is queued
  // or running changes nothing.
  const lone = new tiny_skia.Pixmap(4, 4);
  const rendering = resvg.renderNodeAsync(tree.nodeById('g'), transform, lone);
  tree.dispose();
  assert.equal(await rendering, true);
  assert.equal(lone.width(), 4);

  // A database loading fonts is leased: its copy-on-write value is written.
  const fonts = new fontdb.Database();
  const loading = fonts.loadFontsDirAsync(fileURLToPath(FONT_DIR));
  assert.throws(() => fonts.len(), coded('Leased', 'this Database is in use by an async call'));
  await loading;
  assert.ok(fonts.len() > 0);
}

// 4. The work leaves the event loop: a timer keeps firing during it.
{
  const tree = usvg.Tree.fromStr(HEAVY);
  const ticks = async (fn) => {
    let n = 0;
    const timer = setInterval(() => { n += 1; }, 1);
    await fn();
    clearInterval(timer);
    return n;
  };
  const pixmap = () => new tiny_skia.Pixmap(1600, 1600);
  const big = tiny_skia.Transform.fromScale(4, 4);
  const asyncTicks = await ticks(() => resvg.renderAsync(tree, big, pixmap()));
  const syncTicks = await ticks(async () => resvg.render(tree, big, pixmap()));
  console.log(`  timer ticks during a render: async ${asyncTicks}, sync ${syncTicks}`);
  assert.ok(asyncTicks > 0, 'the event loop turned during renderAsync');
  assert.equal(syncTicks, 0, 'a sync render blocks it');
}

// 5. An AbortSignal drops a call still queued: with one worker thread, a
//    call behind a slow one. It rejects with an AbortError, never runs, and
//    the pixmap it leased is back, blank. A call already running finishes.
{
  const heavy = usvg.Tree.fromStr(HEAVY);
  const blockerPixmap = new tiny_skia.Pixmap(2000, 2000);
  const blocker = resvg.renderAsync(heavy, tiny_skia.Transform.fromScale(5, 5), blockerPixmap);
  const tree = usvg.Tree.fromStr(SVG, options);
  const pixmap = new tiny_skia.Pixmap(120, 80);
  const ac = new AbortController();
  const queued = resvg.renderAsync(tree, transform, pixmap, ac.signal);
  assert.throws(() => pixmap.width(), coded('Leased', /in use by an async call/));
  ac.abort();
  await assert.rejects(queued, (e) => {
    console.log(`  abort rejected with: ${e.name}: ${e.message}`);
    return e.name === 'AbortError';
  });
  assert.equal(pixmap.width(), 120, 'the lease came back');
  assert.ok(pixmap.data().every((b) => b === 0), 'and the render never ran');
  // The running call was untouched: its pixmap comes back drawn on.
  assert.throws(() => blockerPixmap.width(), coded('Leased', /in use/));
  await blocker;
  assert.ok(blockerPixmap.data().some((b) => b !== 0));

  // A queued encode, a queued parse and a queued node render, the same way.
  const again = resvg.renderAsync(heavy, tiny_skia.Transform.fromScale(5, 5), blockerPixmap);
  const ac2 = new AbortController();
  const encode = pixmap.encodePngAsync(ac2.signal);
  const parse = usvg.Tree.fromStrAsync(SVG, null, ac2.signal);
  const node = resvg.renderNodeAsync(tree.nodeById('g'), transform, new tiny_skia.Pixmap(4, 4), ac2.signal);
  ac2.abort();
  for (const p of [encode, parse, node]) await assert.rejects(p, (e) => e.name === 'AbortError');
  assert.equal(pixmap.width(), 120);
  await again;
}

// 6. A signal already aborted is refused before anything is leased: napi
//    only listens for the event, so the whole call would otherwise run.
{
  const gone = new AbortController();
  gone.abort();
  const tree = usvg.Tree.fromStr(SVG);
  const pixmap = new tiny_skia.Pixmap(10, 10);
  const db = new fontdb.Database();
  const calls = {
    fromStrAsync: (s) => usvg.Tree.fromStrAsync(SVG, null, s),
    fromDataAsync: (s) => usvg.Tree.fromDataAsync(Buffer.from(SVG), null, s),
    toStringAsync: (s) => tree.toStringAsync(null, s),
    renderAsync: (s) => resvg.renderAsync(tree, transform, pixmap, s),
    renderNodeAsync: (s) => resvg.renderNodeAsync(tree.root().children()[0], transform, pixmap, s),
    encodePngAsync: (s) => pixmap.encodePngAsync(s),
    loadSystemFontsAsync: (s) => db.loadSystemFontsAsync(s),
    loadFontsDirAsync: (s) => db.loadFontsDirAsync(fileURLToPath(FONT_DIR), s),
  };
  for (const [name, call] of Object.entries(calls)) {
    await assert.rejects(async () => call(gone.signal), /already aborted/, name);
    // A `signal` that is not an AbortSignal is refused before napi converts
    // it (GHSA-qr54-xrr9-7575): one of this module's instances most of all.
    for (const wrong of [pixmap, tree.root(), {}]) {
      await assert.rejects(async () => call(wrong), /signal must be an AbortSignal/, name);
    }
  }
  assert.equal(pixmap.width(), 10, 'nothing was leased');
  assert.equal(db.len(), 0);
  // A live signal still passes.
  assert.ok((await pixmap.encodePngAsync(new AbortController().signal)).length > 0);
}

// 7. Parallel calls share the pool without stepping on each other.
{
  const tree = usvg.Tree.fromStr(SVG, options);
  const pixmaps = [1, 2, 3, 4].map(() => new tiny_skia.Pixmap(120, 80));
  await Promise.all(pixmaps.map((p) => resvg.renderAsync(tree, transform, p)));
  const pngs = await Promise.all(pixmaps.map((p) => p.encodePngAsync()));
  assert.ok(pngs.every((png) => Buffer.compare(png, pngs[0]) === 0));
}
