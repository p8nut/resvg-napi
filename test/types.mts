import { writeFileSync } from 'node:fs';
import {
  Resvg,
  renderAsync,
  FontDatabase,
  ShapeRendering,
  setLogLevel,
  TextRendering,
  type RenderOptions,
  type RawImage,
  type Color,
  type Path,
  type PathSegment,
} from '../index.js';

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="80">
  <rect width="240" height="80" rx="12" fill="#0f172a"/>
  <text x="20" y="50" font-family="sans-serif" font-size="28" fill="#38bdf8">resvg 0.48</text>
</svg>`;

// 1. fonts: opaque class, methods lifted from fontdb::Database by the AST pass
const fonts = new FontDatabase();
fonts.loadSystemFonts();
fonts.setSansSerifFamily('DejaVu Sans');
console.log(`${fonts.len()} font faces, empty=${fonts.isEmpty()}`);

// 2. options: flat JSON object mirrored from usvg::Options, fully typed
const options: RenderOptions = {
  dpi: 192,
  fontFamily: 'DejaVu Sans',
  fontSize: 28,
  languages: ['fr', 'en'],
  // String literals, not enum members: the enums are type-only unions now, so
  // no ambient `const enum` stands between a caller and these fields under
  // isolatedModules -- which is every bundler.
  shapeRendering: 'geometricPrecision',
  textRendering: 'optimizeLegibility',
  styleSheet: 'text { letter-spacing: 1px }',
};

// The union still rejects a typo, which is the point of typing it at all.
const _valid: ShapeRendering = 'crispEdges';
// @ts-expect-error not one of the three
const _bad: ShapeRendering = 'crispedges';
const _level: Parameters<typeof setLogLevel>[0] = 'warn';
// @ts-expect-error not one of the six
const _badLevel: Parameters<typeof setLogLevel>[0] = 'loud';

// 3. parse once, render many
const doc = new Resvg(svg, options, fonts);
console.log(`viewport: ${doc.width}x${doc.height}`);

const png: Buffer = doc.renderPng({ width: 960 });
writeFileSync('out.png', png);

const raw: RawImage = doc.renderRaw({ scale: 2 });
console.log(`png=${png.length}B  raw=${raw.width}x${raw.height} (${raw.data.length}B RGBA)`);

// 4. Buffer input works too, errors arrive as JS exceptions
try {
  new Resvg(Buffer.from('<svg'));
} catch (e) {
  console.log(`rejected: ${(e as Error).message}`);
}

// 5. async: parse and render off the event loop, cancellable
const ctrl = new AbortController();
const pngAsync: Buffer = await renderAsync(svg, options, { width: 480 }, ctrl.signal);
const parsed: Resvg = await Resvg.parseAsync(svg, options, fonts);
const rawAsync: RawImage = await parsed.renderRawAsync({ scale: 1.5 });
console.log(`async: png=${pngAsync.length}B  raw=${rawAsync.width}x${rawAsync.height}`);

// 6. the derived shape of a path node. This section exists to be *compiled*,
// not run: `type` is a string literal in the .d.ts, so narrowing is what proves
// the union is real -- with `type: string` neither branch below would check.
const shape: Path | null | undefined = parsed.node('anything')?.path();
if (shape) {
  const segs: PathSegment[] = shape.data;
  console.log(`${segs.length} segments, first is ${segs[0]?.type}`);

  const paint = shape.fill?.paint;
  if (paint) {
    if (paint.type === 'color') {
      // only the colour branch has `color`
      const { red, green, blue }: Color = paint.value;
      console.log(`fill rgb(${red} ${green} ${blue})`);
    } else {
      // and only the server branch has `id`, with the kind narrowed too
      const kind: 'linearGradient' | 'radialGradient' | 'pattern' = paint.type;
      console.log(`fill is a ${kind} named ${paint.id}`);
    }
    // @ts-expect-error a colour carries a value, not an id
    paint.id?.toString();
  }

  const dashes: number[] | undefined = shape.stroke?.dasharray;
  console.log(`dashes: ${dashes?.join(" ") ?? "none"}`);
}
