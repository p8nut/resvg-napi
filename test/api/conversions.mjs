// The classes that are values: tiny-skia's geometry and colour, svgtypes'
// colour. Their Rust associated functions and methods, and the conversions
// (ADR 0002, Q31, Q34): plain data gets `fromObject`/`toObject`, always both;
// `FromStr` gives `fromStr`; every argument takes the class, never an object.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { inspect } from 'node:util';
import binding from '../../index.js';

const { tiny_skia, svgtypes } = binding;
const { Transform, Rect, Size, IntSize, Color } = tiny_skia;

const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-5, `${a} ≈ ${b}`);

test('Transform: fromObject and toObject round-trip, field by field', () => {
  const o = { sx: 2, kx: 0.5, ky: -0.25, sy: 3, tx: 10, ty: -4 };
  const t = Transform.fromObject(o);
  assert.ok(t instanceof Transform);
  assert.deepEqual(t.toObject(), o);
  assert.deepEqual(Transform.fromObject(t.toObject()).toObject(), o);
  // The public fields are accessors too.
  assert.equal(t.tx, 10);
  t.tx = 7;
  assert.equal(t.toObject().tx, 7);
  // Every field is required; a value is a class or nothing.
  assert.throws(() => Transform.fromObject({ sx: 1 }));
  assert.throws(() => new Transform(), 'no `new` in Rust: no constructor');
});

test('Transform: the Rust associated functions and methods', () => {
  assert.equal(Transform.identity().isIdentity(), true);
  assert.deepEqual(Transform.fromScale(2, 3).toObject(), { sx: 2, kx: 0, ky: 0, sy: 3, tx: 0, ty: 0 });
  assert.deepEqual(Transform.fromTranslate(5, 6).toObject(), { sx: 1, kx: 0, ky: 0, sy: 1, tx: 5, ty: 6 });
  // `from_row(sx, ky, kx, sy, tx, ty)`: tiny-skia's argument order.
  assert.deepEqual(Transform.fromRow(1, 2, 3, 4, 5, 6).toObject(), { sx: 1, ky: 2, kx: 3, sy: 4, tx: 5, ty: 6 });
  const t = Transform.fromScale(2, 2).preTranslate(1, 1);
  assert.deepEqual(t.toObject(), { sx: 2, kx: 0, ky: 0, sy: 2, tx: 2, ty: 2 });
  assert.deepEqual(Transform.fromScale(2, 2).postTranslate(1, 1).toObject(), { sx: 2, kx: 0, ky: 0, sy: 2, tx: 1, ty: 1 });
  const r = Transform.fromRotate(90);
  near(r.ky, 1);
  near(r.kx, -1);
  assert.equal(r.hasSkew(), true);
  // Methods take `&self` and return a new value: the receiver is unchanged.
  const s = Transform.fromScale(2, 2);
  s.preScale(3, 3);
  assert.equal(s.sx, 2);
  const inv = t.invert();
  assert.deepEqual(t.preConcat(inv).toObject(), Transform.identity().toObject());
  assert.equal(Transform.fromScale(2, 2).isScale(), true);
  assert.equal(Transform.fromTranslate(1, 0).isTranslate(), true);
});

test('Rect, Size and IntSize: factories and methods, no fields to copy', () => {
  const r = Rect.fromXywh(1, 2, 10, 20);
  assert.deepEqual([r.left(), r.top(), r.right(), r.bottom(), r.width(), r.height()], [1, 2, 11, 22, 10, 20]);
  const moved = r.transform(Transform.fromTranslate(5, 5));
  assert.deepEqual([moved.x(), moved.y()], [6, 7]);
  assert.deepEqual([r.x(), r.y()], [1, 2], 'the receiver is unchanged');
  const j = r.join(Rect.fromLtrb(0, 0, 1, 1));
  assert.deepEqual([j.left(), j.top(), j.right(), j.bottom()], [0, 0, 11, 22]);
  assert.equal(r.intersect(Rect.fromLtrb(100, 100, 101, 101)), null);
  const s = Size.fromWh(4, 3);
  assert.deepEqual([s.width(), s.height()], [4, 3]);
  const toW = s.scaleToWidth(8);
  assert.deepEqual([toW.width(), toW.height()], [8, 6]);
  const rect = s.toRect(1, 1);
  assert.ok(rect instanceof Rect);
  assert.equal(rect.right(), 5);
  const i = s.toIntSize();
  assert.ok(i instanceof IntSize);
  assert.deepEqual([i.width(), i.height()], [4, 3]);
  assert.deepEqual([IntSize.fromWh(2, 5).scaleBy(2).width(), i.toSize().height()], [4, 3]);
  // Private fields in Rust: no fromObject/toObject.
  for (const c of [Rect, Size, IntSize, Color]) {
    assert.equal(c.fromObject, undefined, c.name);
    assert.equal(c.prototype.toObject, undefined, c.name);
  }
});

test('tiny_skia.Color: accessors and setters as Rust has them', () => {
  const c = Color.fromRgba8(255, 0, 51, 255);
  assert.deepEqual([c.red(), c.green(), c.alpha()], [1, 0, 1]);
  near(c.blue(), 0.2);
  assert.equal(c.isOpaque(), true);
  c.setAlpha(0.5);
  assert.equal(c.alpha(), 0.5);
  c.applyOpacity(0.5);
  assert.equal(c.alpha(), 0.25);
});

test('svgtypes.Color: fromStr parses CSS colours, plain data both ways', () => {
  assert.deepEqual(svgtypes.Color.fromStr('#fff').toObject(), { red: 255, green: 255, blue: 255, alpha: 255 });
  assert.deepEqual(svgtypes.Color.fromStr('rgba(10, 20, 30, 0.5)').toObject(), { red: 10, green: 20, blue: 30, alpha: 128 });
  assert.deepEqual(svgtypes.Color.fromStr('teal').toObject(), { red: 0, green: 128, blue: 128, alpha: 255 });
  const o = { red: 1, green: 2, blue: 3, alpha: 4 };
  assert.deepEqual(svgtypes.Color.fromObject(o).toObject(), o);
  const c = svgtypes.Color.newRgb(1, 2, 3);
  assert.equal(c.alpha, 255);
  c.alpha = 7;
  assert.equal(c.toObject().alpha, 7);
  // A static named like a field: `Color.red()` is pure red, `color.red` a channel.
  assert.equal(svgtypes.Color.red().red, 255);
  assert.equal(svgtypes.Color.red().green, 0);
  // One direction: no `Display` upstream, so no `toString` of its own.
  assert.equal(Object.hasOwn(svgtypes.Color.prototype, 'toString'), false);
});

test('two Colors, one name: each namespace builds its own', () => {
  const a = Color.fromRgba8(1, 2, 3, 4);
  const b = svgtypes.Color.fromStr('red');
  assert.ok(a instanceof tiny_skia.Color && !(a instanceof svgtypes.Color));
  assert.ok(b instanceof svgtypes.Color && !(b instanceof tiny_skia.Color));
  for (const c of [svgtypes.Color.black(), svgtypes.Color.fromObject({ red: 0, green: 0, blue: 0, alpha: 0 }),
    svgtypes.Color.newRgba(0, 0, 0, 0)]) {
    assert.ok(c instanceof svgtypes.Color);
  }
  // `Color::from_rgba` returns `Option<Self>`: napi would build that from
  // the constructor registered under `Color`, which is one of the two, so it
  // is left out (codegen-report.txt says so).
  assert.equal(Color.fromRgba, undefined);
  // From an svgtypes colour to a tiny-skia one, as the helpers will.
  const css = svgtypes.Color.fromStr('#336699');
  const fill = Color.fromRgba8(css.red, css.green, css.blue, css.alpha);
  assert.equal(Math.round(fill.green() * 255), 0x66);
});

test('inspect shows the fields and the cheap &self methods', () => {
  assert.match(inspect(Transform.identity()), /^Transform \{.*sx: 1.*isIdentity: true/s);
  assert.match(inspect(svgtypes.Color.black()), /^Color \{.*red: 0.*alpha: 255/s);
  assert.match(inspect(new tiny_skia.Pixmap(3, 2)), /^Pixmap \{ width: 3, height: 2 \}$/);
});
