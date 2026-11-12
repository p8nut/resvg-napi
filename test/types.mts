// Compile-time and run-time check of index.d.ts: the namespaced API as a
// TypeScript caller writes it. (Until 0.10 this began with the flat `Resvg`.)
import {
  setLogLevel,
  usvg,
  fontdb,
  tiny_skia,
  svgtypes,
  resvg,
} from '../index.js';

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="80">
  <rect width="240" height="80" rx="12" fill="#0f172a"/>
  <path id="p" d="M10 10 L50 10 Q60 30 30 50 Z" fill="#38bdf8"/>
  <text x="20" y="50" font-family="sans-serif" font-size="28" fill="#38bdf8">resvg 0.49</text>
</svg>`;

// 1. fonts: a class, with fontdb::Database's methods
const fonts = new fontdb.Database();
fonts.loadSystemFonts();
fonts.setSansSerifFamily('DejaVu Sans');
console.log(`${fonts.len()} font faces, empty=${fonts.isEmpty()}`);

// 2. options: a plain object mirrored from usvg::Options, fully typed
const options: usvg.Options = {
  fontdb: fonts,
  dpi: 192,
  fontFamily: 'DejaVu Sans',
  fontSize: 28,
  languages: ['fr', 'en'],
  // String literals, not enum members: the enums are type-only unions, so no
  // ambient `const enum` stands between a caller and these fields under
  // isolatedModules -- which is every bundler.
  shapeRendering: 'geometricPrecision',
  textRendering: 'optimizeLegibility',
  styleSheet: 'text { letter-spacing: 1px }',
  // A `tiny_skia.Size`, built by its factory (Q39).
  defaultSize: tiny_skia.Size.fromWh(100, 100)!,
};
// @ts-expect-error `defaultSize` is a `tiny_skia.Size`, not its numbers
const _flatSize: usvg.Options = { defaultSize: { width: 1, height: 1 } };

// The union still rejects a typo, which is the point of typing it at all.
const _valid: usvg.ShapeRendering = 'crispEdges';
// @ts-expect-error not one of the three
const _bad: usvg.ShapeRendering = 'crispedges';
const _level: Parameters<typeof setLogLevel>[0] = 'warn';
// @ts-expect-error not one of the six
const _badLevel: Parameters<typeof setLogLevel>[0] = 'loud';

// 3. parse once, render many
const tree = usvg.Tree.fromStr(svg, options);
const size: tiny_skia.Size = tree.size();
console.log(`viewport: ${size.width()}x${size.height()}`);
const pixmap = new tiny_skia.Pixmap(Math.ceil(size.width()), Math.ceil(size.height()));
resvg.render(tree, tiny_skia.Transform.identity(), pixmap);
const png: Buffer = pixmap.encodePng();
console.log(`png=${png.length}B`);

// 4. boxes are tiny-skia's classes, null for a node with no content (an
//    empty group, since usvg 0.50), and a `Rect` converts.
const layer: tiny_skia.NonZeroRect | null = tree.root().absLayerBoundingBox();
const stroke: tiny_skia.Rect | null = tree.root().absStrokeBoundingBox();
const maybe: tiny_skia.NonZeroRect | null | undefined = stroke?.toNonZeroRect();
console.log(layer?.width(), maybe?.height());

// 5. a face's families are `[name, language]` pairs (Q38)
for (const face of fonts.faces()) {
  const [[name, language]]: Array<[string, fontdb.Language]> = face.families;
  console.log(name, language);
  break;
}

// 6. the derived shape of a path node. This section exists to be *compiled*,
// not run: `type` is a string literal in the .d.ts, so narrowing is what proves
// the union is real -- with `type: string` neither branch below would check.
const found = usvg.Tree.fromStr(svg).nodeById('p');
// @ts-expect-error a node is a union: only a path's value has `data()`
found?.value.data();
if (found?.kind === 'path') {
  const shape: usvg.Path = found.value;
  const segs = shape.data();
  // `tiny_skia_path::PathSegment` as a union: narrowing on `type` gives the
  // payload, one point or a tuple of them.
  for (const seg of segs) {
    if (seg.type === 'quadTo') {
      const [control, end]: [tiny_skia.PointObject, tiny_skia.PointObject] = seg.value;
      console.log(control.x, end.y);
    } else if (seg.type === 'moveTo' || seg.type === 'lineTo') {
      const at: tiny_skia.PointObject = seg.value;
      console.log(at.x);
    }
  }
  console.log(`${segs.length} segments, first is ${segs[0]?.type}`);

  const paint = shape.fill()?.paint;
  if (paint) {
    if (paint.type === 'color') {
      // only the colour branch has `color`
      const { red, green, blue }: usvg.Color = paint.value;
      console.log(`fill rgb(${red} ${green} ${blue})`);
    } else {
      // and only the server branch has `id`, with the kind narrowed too
      const kind: 'linearGradient' | 'radialGradient' | 'pattern' = paint.type;
      console.log(`fill is a ${kind} named ${paint.id}`);
    }
    // @ts-expect-error a colour carries a value, not an id
    paint.id?.toString();
  }

  const dashes: number[] | undefined = shape.stroke()?.dasharray;
  console.log(`dashes: ${dashes?.join(" ") ?? "none"}`);
}

// 7. a parse failure's `code` is one of usvg's own variant names. The switch
//    is exhaustive: a code added upstream fails to type-check here.
try {
  usvg.Tree.fromStr('<svg');
} catch (e) {
  const code = (e as { code?: usvg.ErrorCode }).code;
  switch (code) {
    case 'NotAnUtf8Str':
    case 'SvgzFeatureNotEnabled':
    case 'MalformedGZip':
    case 'ElementsLimitReached':
    case 'InvalidSize':
    case 'ParsingFailed':
    case undefined:
      console.log(`parse failed: ${code}`);
      break;
    default: {
      const unhandled: never = code;
      throw new Error(`unhandled ${unhandled}`);
    }
  }
}

// 9. strings are `string`, not the `String` wrapper object type, and binary
//    inputs take any Uint8Array -- what fetch hands back in a browser or WASI.
for (const face of fonts.faces()) {
  const name: string = face.postScriptName;
  new Map<string, number>().set(face.postScriptName, name.length);
}
const bytes = new Uint8Array(Buffer.from(svg));
usvg.Tree.fromData(bytes);
fonts.loadFontData(new Uint8Array(0));

// 10. the namespaced option objects: every field optional, indent in its JS
//     form, and `fontdb` only a `fontdb.Database` instance (a provider).
const nsOptions: usvg.Options = {};
const nsWrite: usvg.WriteOptions = { indent: 'tabs', attributesIndent: 2, coordinatesPrecision: 3 };
// @ts-expect-error a plain object is not a `fontdb.Database`
const nsBadFonts: usvg.Options = { fontdb: {} };
void [nsOptions, nsWrite, nsBadFonts];
// @ts-expect-error a shared file carries a path and its bytes: it cannot be passed in
const nsShared: fontdb.Source = { type: 'sharedFile', value: '/x.ttf' };
void nsShared;

// 11. the Rust render loop, typed: factories, a free function, the classes
//     as arguments, `T | null` for an Option, the codes of a thrown error.
function renderNs(bytes: Uint8Array): Buffer {
  const db = new fontdb.Database();
  db.loadFontData(bytes);
  const tree: usvg.Tree = usvg.Tree.fromData(bytes, { fontdb: db });
  const size: tiny_skia.Size = tree.size();
  const pixmap = new tiny_skia.Pixmap(Math.ceil(size.width()), Math.ceil(size.height()));
  const css = svgtypes.Color.fromStr('#fff');
  pixmap.fill(tiny_skia.Color.fromRgba8(css.red, css.green, css.blue, css.alpha));
  resvg.render(tree, tiny_skia.Transform.fromScale(2, 2), pixmap);
  // @ts-expect-error a plain object is not a `tiny_skia.Transform`
  resvg.render(tree, { sx: 1, kx: 0, ky: 0, sy: 1, tx: 0, ty: 0 }, pixmap);
  const t: tiny_skia.TransformObject = tiny_skia.Transform.fromObject({ sx: 1, kx: 0, ky: 0, sy: 1, tx: 0, ty: 0 }).toObject();
  const inverse: tiny_skia.Transform | null = tiny_skia.Transform.fromObject(t).invert();
  const code: usvg.ErrorCode = 'ParsingFailed';
  void [inverse, code, tree.toString({ indent: 2 })];
  return pixmap.encodePng();
}
void renderNs;

// 12. every class is a `Disposable`, what `using` asks of its value,
//     declared through a generated header that merges `[Symbol.dispose]`
//     into each class, under its namespace. No `using` statement here: this
//     file also runs, and Node parses one only from 24.
new tiny_skia.Pixmap(1, 1) satisfies Disposable;
usvg.Tree.fromStr(svg) satisfies Disposable;
usvg.Tree.fromStr(svg).root() satisfies Disposable;
new fontdb.Database() satisfies Disposable;
svgtypes.Color.fromStr('red') satisfies Disposable;
const gone: boolean = tiny_skia.Transform.identity().disposed;
void gone;

// 13. the async twins: a promise of what the sync call returns, `signal` last.
async function renderNsAsync(bytes: Uint8Array, signal: AbortSignal): Promise<Buffer> {
  const db = new fontdb.Database();
  const loaded: void = await db.loadFontsDirAsync('fonts', signal);
  const tree: usvg.Tree = await usvg.Tree.fromDataAsync(bytes, { fontdb: db }, signal);
  const pixmap = new tiny_skia.Pixmap(10, 10);
  const drawn: void = await resvg.renderAsync(tree, tiny_skia.Transform.identity(), pixmap);
  const node: boolean | null = await resvg.renderNodeAsync(tree.root().children()[0], tiny_skia.Transform.identity(), pixmap);
  const text: string = await tree.toStringAsync({ indent: 2 }, signal);
  // @ts-expect-error a signal is an `AbortSignal`
  await tree.toStringAsync(null, {});
  void [loaded, drawn, node, text];
  return pixmap.encodePngAsync(signal);
}
void renderNsAsync;
