// What the namespaced surface throws (ADR 0002, Q23, Q24): a `Result` whose
// error is an upstream enum throws an `Error` with `code`, the variant's name,
// and `message`, its `Display` unchanged; `new` returning `None` throws.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { inspect } from 'node:util';
import { gzipSync } from 'node:zlib';
import binding from '../../index.js';

const { usvg, fontdb, tiny_skia, svgtypes } = binding;

/** `fn` throws an `Error` with this `code`, and a `message` matching. */
function throwsCoded(fn, code, message) {
  assert.throws(fn, (e) => {
    assert.ok(e instanceof Error, 'an Error');
    assert.equal(e.code, code);
    if (message instanceof RegExp) assert.match(e.message, message);
    else assert.equal(e.message, message);
    return true;
  });
}

test('usvg parse errors: the variant as code, usvg\'s own message', () => {
  throwsCoded(() => usvg.Tree.fromStr('<svg'), 'ParsingFailed', /^SVG data parsing failed cause /);
  throwsCoded(() => usvg.Tree.fromData(new Uint8Array([0xff, 0xfe, 0x3c])), 'NotAnUtf8Str',
    'provided data has not an UTF-8 encoding');
  throwsCoded(() => usvg.Tree.fromStr('<svg xmlns="http://www.w3.org/2000/svg" width="0" height="10"/>'),
    'InvalidSize', 'SVG has an invalid size');
  // No `svgz` feature: usvg's own advice, which the helpers will reword.
  const svgz = gzipSync('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>');
  throwsCoded(() => usvg.Tree.fromData(svgz), 'SvgzFeatureNotEnabled', /svgz/i);
});

test('each error enum has a type of its codes', () => {
  assert.deepEqual(Object.values(usvg.ErrorCode).sort(), [
    'ElementsLimitReached', 'InvalidSize', 'MalformedGZip', 'NotAnUtf8Str', 'ParsingFailed',
    'SvgzFeatureNotEnabled',
  ]);
  assert.ok(Object.values(svgtypes.ErrorCode).includes('InvalidValue'));
});

test('new returning None throws: a JS constructor cannot return null', () => {
  throwsCoded(() => new tiny_skia.Pixmap(0, 0), 'InvalidArg', /Pixmap::new returned None/);
  throwsCoded(() => new tiny_skia.Pixmap(10, 0), 'InvalidArg', /Pixmap::new returned None/);
  // Any other Option is null.
  assert.equal(tiny_skia.IntSize.fromWh(0, 1), null);
  assert.equal(tiny_skia.Rect.fromLtrb(10, 0, 0, 10), null);
  assert.equal(tiny_skia.Pixmap.fromVec(new Uint8Array(3), tiny_skia.IntSize.fromWh(1, 1)), null);
});

test('svgtypes.Color.fromStr throws on what does not parse', () => {
  throwsCoded(() => svgtypes.Color.fromStr('nope'), 'InvalidValue', 'invalid value');
  throwsCoded(() => svgtypes.Color.fromStr(''), 'UnexpectedEndOfStream', 'unexpected end of stream');
  throwsCoded(() => svgtypes.Color.fromStr('#ff'), 'InvalidValue', 'invalid value');
});

test('encodePng and decodePng: an error type no source defines is GenericFailure', () => {
  const pixmap = new tiny_skia.Pixmap(2, 2);
  const png = pixmap.encodePng();
  const back = tiny_skia.Pixmap.decodePng(png);
  assert.deepEqual([back.width(), back.height()], [2, 2]);
  throwsCoded(() => tiny_skia.Pixmap.decodePng(new Uint8Array([1, 2, 3])), 'GenericFailure', /./);
  throwsCoded(() => tiny_skia.Pixmap.loadPng('/nonexistent/x.png'), 'GenericFailure', /./);
  throwsCoded(() => new fontdb.Database().loadFontFile('/nonexistent/x.ttf'), 'GenericFailure', /./);
});

test('a consumed pixmap throws on every later call', () => {
  const pixmap = new tiny_skia.Pixmap(1, 1);
  assert.equal(pixmap.takeDemultiplied().length, 4);
  for (const call of [() => pixmap.encodePng(), () => pixmap.take(), () => pixmap.data(),
    () => pixmap.fill(tiny_skia.Color.fromRgba8(0, 0, 0, 0))]) {
    throwsCoded(call, 'Consumed', 'this Pixmap was consumed by an earlier call');
  }
  assert.equal(inspect(pixmap), 'Pixmap { consumed: true }');
});

test('inspect rethrows what a member threw, even a thrown null', () => {
  for (const thrown of [null, undefined, 'text', Object.assign(new Error('x'), { code: 7 })]) {
    class Odd extends tiny_skia.Pixmap { width() { throw thrown; } }
    assert.throws(() => inspect(new Odd(1, 1)), (e) => e === thrown);
  }
});

test('a number that does not fit the upstream integer throws, never wraps', () => {
  throwsCoded(() => tiny_skia.Color.fromRgba8(256, 0, 0, 0), 'InvalidArg', 'invalid r: 256');
  throwsCoded(() => svgtypes.Color.newRgb(0, -1, 0), 'InvalidArg', /green|u32|number/);
  assert.throws(() => new tiny_skia.Pixmap(-1, 1));
});

test('arguments are checked: the class, never a look-alike', () => {
  const pixmap = new tiny_skia.Pixmap(1, 1);
  assert.throws(() => pixmap.fill({ r: 0, g: 0, b: 0, a: 1 }));
  assert.throws(() => pixmap.fill(svgtypes.Color.black()), 'svgtypes.Color is not tiny_skia.Color');
  assert.throws(() => usvg.Tree.fromData('<svg/>'), 'fromData takes bytes; fromStr a string');
});

// Ported from the 0.9 suite (test/errors.mjs): an instance of another class
// where a class is expected is refused, not reinterpreted. Unchecked, a
// `usvg.Path` read as a `tiny_skia.Transform` is memory corruption. Native
// (type tags) and WASI (the class recorded at wrap time) alike.
test('another class\'s instance is refused where a class is expected', () => {
  const tree = usvg.Tree.fromStr('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect id="r" width="1" height="1"/></svg>');
  const path = tree.nodeById('r').value;
  const pixmap = new tiny_skia.Pixmap(1, 1);
  const refused = (cls) => (e) => e instanceof Error && e.code === 'InvalidArg'
    && new RegExp(`not an instance of class \`${cls}\``).test(e.message);
  assert.throws(() => binding.resvg.render(tree, path, pixmap), refused('TinySkiaTransform'));
  assert.throws(() => binding.resvg.render(path, tiny_skia.Transform.identity(), pixmap), refused('UsvgTree'));
  assert.throws(() => usvg.Tree.fromStr('<svg/>', { fontdb: path }), /Database/);
});
