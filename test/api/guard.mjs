// The `guard` providers (ADR 0001, "Safety"; ADR 0002, Q26; step 10 of
// docs/migration-0.10.md): `usvg.Options.imageHrefResolver` and
// `fontResolver` take a `guard.ImageHrefResolver` and a `guard.FontResolver`.
// Without them the raw API resolves as Rust does, reading any path: the
// first test shows it, so that the opt-in is real. The helpers pass a fresh
// one of each to every parse (test/helpers/parse.mjs, test/images.mjs,
// test/limits.mjs).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, symlinkSync, truncateSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { Readable } from 'node:stream';
import { buffer } from 'node:stream/consumers';
import zlib from 'node:zlib';
import binding from '../../index.js';

const { usvg, fontdb, tiny_skia, resvg, guard, setLogLevel, takeLogs } = binding;
const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));

/** A 20x20 PNG of one colour. */
function png(r, g, b) {
  const p = new tiny_skia.Pixmap(20, 20);
  p.fill(tiny_skia.Color.fromRgba8(r, g, b, 255));
  return p.encodePng();
}
const RED = png(255, 0, 0);

const doc = (href) => `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">
  <image xlink:href="${href}" x="0" y="0" width="20" height="20"/>
</svg>`;

/** The first pixel of `tree` rendered at its size. */
function pixel(tree) {
  const pixmap = new tiny_skia.Pixmap(20, 20);
  resvg.render(tree, tiny_skia.Transform.identity(), pixmap);
  return [...pixmap.data().subarray(0, 4)];
}

/** Parses `svg` with a fresh resolver of `options`: the tree and its misses. */
function guarded(svg, options, parse = {}) {
  const imageHrefResolver = new guard.ImageHrefResolver(options);
  const tree = usvg.Tree.fromStr(svg, { ...parse, imageHrefResolver });
  return { tree, misses: imageHrefResolver.misses() };
}

/** A file holding `RED` in a fresh directory, and a sibling directory. */
function outsideAndInside() {
  const outside = mkdtempSync(join(tmpdir(), 'resvg-out-'));
  const secret = join(outside, 'secret.png');
  writeFileSync(secret, RED);
  const inside = mkdtempSync(join(tmpdir(), 'resvg-in-'));
  return { outside, secret, inside };
}

test('without a provider the raw API reads any path, as Rust does; the guard does not', () => {
  const { secret, inside } = outsideAndInside();
  // usvg's default resolver: an absolute href is read, whatever resourcesDir.
  for (const options of [undefined, { resourcesDir: inside }]) {
    assert.deepEqual(pixel(usvg.Tree.fromStr(doc(secret), options)), [255, 0, 0, 255], 'the raw default read it');
  }
  // The guard: refused, and reported.
  for (const [options, parse] of [[{}, {}], [{ resourcesDir: inside }, {}], [{}, { resourcesDir: inside }]]) {
    const { tree, misses } = guarded(doc(secret), options, parse);
    assert.deepEqual(pixel(tree), [0, 0, 0, 0], 'nothing drawn');
    assert.deepEqual(misses, [secret]);
  }
});

test('supplied images first, then data URLs, nested SVG sniffed', () => {
  const supplied = guarded(doc('company-logo'), { images: { 'company-logo': RED } });
  assert.deepEqual(supplied.misses, []);
  assert.deepEqual(pixel(supplied.tree), [255, 0, 0, 255]);

  const data = guarded(doc(`data:image/png;base64,${RED.toString('base64')}`));
  assert.deepEqual(data.misses, []);
  assert.deepEqual(pixel(data.tree), [255, 0, 0, 255], 'data URL');

  const green = '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="#00ff00"/></svg>';
  const nested = guarded(doc('sub'), { images: { sub: Buffer.from(green) } });
  assert.deepEqual(nested.misses, []);
  assert.deepEqual(pixel(nested.tree), [0, 255, 0, 255], 'nested SVG drawn');

  const missing = guarded(doc('company-logo'));
  assert.deepEqual(missing.misses, ['company-logo'], 'unresolved, reported');
  assert.deepEqual(pixel(missing.tree), [0, 0, 0, 0]);
});

test('an href is a miss once; unusable bytes are a miss', () => {
  const twice = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">
    <image id="i" href="a.png" width="5" height="5"/><image href="a.png" width="5" height="5"/><use xlink:href="#i"/></svg>`;
  assert.deepEqual(guarded(twice).misses, ['a.png'], 'deduplicated');
  assert.deepEqual(guarded(twice, { images: { 'a.png': Buffer.from('not an image') } }).misses, ['a.png']);
});

test('files: only under resourcesDir, resourcesRoot widens the fence', () => {
  const { outside, secret, inside } = outsideAndInside();
  writeFileSync(join(inside, 'disk.png'), RED);
  // The resolver's own resourcesDir, or the parse options'.
  for (const [options, parse] of [[{ resourcesDir: inside }, {}], [{}, { resourcesDir: inside }]]) {
    const { tree, misses } = guarded(doc('disk.png'), options, parse);
    assert.deepEqual(misses, []);
    assert.deepEqual(pixel(tree), [255, 0, 0, 255], 'loaded from disk');
  }
  // The resolver's wins over the options'.
  assert.deepEqual(guarded(doc('disk.png'), { resourcesDir: outside }, { resourcesDir: inside }).misses, ['disk.png']);
  // Neither: no file is read, not even off the working directory.
  assert.deepEqual(guarded(doc('disk.png')).misses, ['disk.png']);

  const up = `../${basename(outside)}/secret.png`;
  const hrefs = [secret, up];
  // Windows needs a privilege to create symlinks; skip that case there.
  try { symlinkSync(secret, join(inside, 'link.png')); hrefs.push('link.png'); } catch {}
  for (const href of hrefs) {
    assert.deepEqual(guarded(doc(href), { resourcesDir: inside }).misses, [href], `refused: ${href}`);
  }
  const root = dirname(outside);
  assert.deepEqual(guarded(doc(up), { resourcesDir: inside, resourcesRoot: root }).misses, [], 'reached under resourcesRoot');
  assert.deepEqual(guarded(doc(up), { resourcesDir: inside, resourcesRoot: inside }).misses, [up]);
  assert.deepEqual(guarded(doc(secret), { resourcesDir: inside, resourcesRoot: inside }).misses, [secret]);
});

test('a file past 256 MiB is refused from its size alone', () => {
  const dir = mkdtempSync(join(tmpdir(), 'resvg-big-'));
  writeFileSync(join(dir, 'big.png'), RED);
  // Sparse: the test costs no disk, and a read ignoring the cap would
  // allocate all of it.
  truncateSync(join(dir, 'big.png'), 256 * 1024 * 1024 + 1);
  assert.deepEqual(guarded(doc('big.png'), { resourcesDir: dir }).misses, ['big.png']);
});

// Only regular files: a FIFO with no writer would block the read forever,
// and no in-process timeout interrupts native code, hence the child process.
test('a FIFO is refused, not read (Linux: mkfifo)', { skip: process.platform !== 'linux' && 'Linux only' }, () => {
  const dir = mkdtempSync(join(tmpdir(), 'resvg-fifo-'));
  execFileSync('mkfifo', [join(dir, 'pipe.png')]);
  const script = `
    const { usvg, guard } = require(${JSON.stringify(createRequire(import.meta.url).resolve('../../index.js'))});
    const doc = ${doc};
    const r = new guard.ImageHrefResolver({ resourcesDir: ${JSON.stringify(dir)} });
    usvg.Tree.fromStr(doc('pipe.png'), { imageHrefResolver: r });
    process.stdout.write(JSON.stringify(r.misses()));`;
  const child = spawnSync(process.execPath, ['-e', script], { timeout: 5000, encoding: 'utf8' });
  assert.equal(child.signal, null, 'parse blocked reading a stream');
  assert.equal(child.status, 0, child.stderr);
  assert.deepEqual(JSON.parse(child.stdout), ['pipe.png']);
});

test('SVGZ is refused, supplied, as a data URL or nested; refusals logged', async () => {
  const MiB = 1024 * 1024;
  // A 300 MiB bomb, streamed so the test never holds it.
  const bomb = await buffer(Readable.from((function* () {
    yield Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><!--');
    const chunk = Buffer.alloc(MiB, 32);
    for (let i = 0; i < 300; i++) yield chunk;
    yield Buffer.from('--></svg>');
  })()).pipe(zlib.createGzip({ level: 9 })));
  const red = '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="red"/></svg>';
  setLogLevel('warn');
  takeLogs();
  try {
    const supplied = guarded(doc('red.svgz'), { images: { 'red.svgz': zlib.gzipSync(red) } });
    assert.deepEqual(supplied.misses, ['red.svgz']);
    assert.deepEqual(pixel(supplied.tree), [0, 0, 0, 0]);
    assert.deepEqual(guarded(doc('bomb.svgz'), { images: { 'bomb.svgz': bomb } }).misses, ['bomb.svgz']);
    const nested = zlib.gzipSync(bomb);
    const uri = guarded(doc(`data:image/svg+xml;base64,${nested.toString('base64')}`));
    assert.deepEqual(pixel(uri.tree), [0, 0, 0, 0], 'svgz data URL not drawn');
    assert.ok(takeLogs().filter((l) => /image refused, SVGZ is not supported/.test(l)).length >= 3, 'logged');
  } finally {
    setLogLevel('off');
  }
});

test('a raster image declaring more than 2^28 px, or no size, is refused', () => {
  const crc = (buf) => {
    let c = ~0;
    for (const b of buf) { c ^= b; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
    return ~c >>> 0;
  };
  const chunk = (type, data) => {
    const out = Buffer.alloc(12 + data.length);
    out.writeUInt32BE(data.length, 0);
    out.write(type, 4, 'latin1');
    data.copy(out, 8);
    out.writeUInt32BE(crc(out.subarray(4, 8 + data.length)), 8 + data.length);
    return out;
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(65535, 0);
  ihdr.writeUInt32BE(65535, 4);
  ihdr.set([8, 6, 0, 0, 0], 8);
  const huge = Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IEND', Buffer.alloc(0))]);
  setLogLevel('warn');
  takeLogs();
  try {
    assert.deepEqual(guarded(doc('huge.png'), { images: { 'huge.png': huge } }).misses, ['huge.png']);
    // A data URL is no way round it: drawn as nothing, not decoded.
    assert.deepEqual(pixel(guarded(doc(`data:image/png;base64,${huge.toString('base64')}`)).tree), [0, 0, 0, 0]);
    assert.ok(takeLogs().filter((l) => /image refused, 4294836225 px exceeds 268435456 px/.test(l)).length >= 2, 'logged');
    const headless = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 0, 0]);
    assert.deepEqual(guarded(doc('headless.png'), { images: { 'headless.png': headless } }).misses, ['headless.png']);
  } finally {
    setLogLevel('off');
  }
});

test('guard.FontResolver records the named families the database lacks', () => {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const text = (families) => `<svg xmlns="http://www.w3.org/2000/svg" width="80" height="30">
    <text x="2" y="22" font-family="${families}" font-size="20">Hi</text>
    <text x="2" y="22" font-family="Ghost Two" font-size="20">Hi</text></svg>`;
  const fontResolver = new guard.FontResolver();
  usvg.Tree.fromStr(text("'Ghost One', 'IBM Plex Sans', sans-serif"), { fontdb: db, fontResolver });
  // Generic families always map to something; a known family is no miss.
  assert.deepEqual(fontResolver.misses(), ['Ghost One', 'Ghost Two']);
  // Font selection is usvg's own: the text still lays out with the known face.
  const fresh = new guard.FontResolver();
  const tree = usvg.Tree.fromStr(text('IBM Plex Sans'), { fontdb: db, fontResolver: fresh });
  assert.deepEqual(fresh.misses(), ['Ghost Two']);
  assert.equal(tree.root().children()[0].kind, 'text');
});

test('async: the resolvers go to the worker, their misses are read after await', async () => {
  const { secret, inside } = outsideAndInside();
  const imageHrefResolver = new guard.ImageHrefResolver({ resourcesDir: inside });
  const fontResolver = new guard.FontResolver();
  const svg = doc(secret).replace('</svg>', '<text font-family="Ghost">x</text></svg>');
  const tree = await usvg.Tree.fromDataAsync(Buffer.from(svg), { imageHrefResolver, fontResolver });
  assert.deepEqual(pixel(tree), [0, 0, 0, 0], 'the fence holds on the worker');
  assert.deepEqual(imageHrefResolver.misses(), [secret]);
  assert.deepEqual(fontResolver.misses(), ['Ghost']);
  const supplied = new guard.ImageHrefResolver({ images: { logo: RED } });
  assert.deepEqual(pixel(await usvg.Tree.fromStrAsync(doc('logo'), { imageHrefResolver: supplied })), [255, 0, 0, 255]);
});

test('one instance per parse: reused, it lists the misses of every parse', async () => {
  const shared = new guard.ImageHrefResolver();
  await Promise.all([
    usvg.Tree.fromStrAsync(doc('a.png'), { imageHrefResolver: shared }),
    usvg.Tree.fromStrAsync(doc('b.png'), { imageHrefResolver: shared }),
  ]);
  assert.deepEqual(shared.misses().sort(), ['a.png', 'b.png']);
});

test('a provider field takes its class only, and not once disposed', () => {
  assert.throws(() => usvg.Tree.fromStr(doc('x'), { imageHrefResolver: new guard.FontResolver() }));
  assert.throws(() => usvg.Tree.fromStr(doc('x'), { fontResolver: new guard.ImageHrefResolver() }));
  assert.throws(() => usvg.Tree.fromStr(doc('x'), { imageHrefResolver: {} }));
  const r = new guard.ImageHrefResolver({ images: { x: RED } });
  r.dispose();
  assert.throws(() => usvg.Tree.fromStr(doc('x'), { imageHrefResolver: r }), { code: 'Disposed' });
  assert.throws(() => r.misses(), { code: 'Disposed', message: 'this ImageHrefResolver was disposed' });
});
