// Compile-time check that helpers.d.mts and helpers.browser.d.mts describe
// helpers.mjs and helpers.browser.mjs. Hand-written, so they can drift; this
// uses every member the way test/helpers/ does at run time.
import helpers, {
  MAX_PIXELS, createHelpers, fitTextWidths, measureText, nodeExtent, parse, parseAsync,
  renderNodePng, renderNodePngAsync, renderPng, renderPngAsync, renderRaw, renderRawAsync,
  systemFonts, systemFontsAsync,
} from '../helpers.mjs';
import * as browser from '../helpers.browser.mjs';
import type { Helpers, Parsed, RawImage, FitResult, RenderOptions } from '../helpers.browser.mjs';
import binding from '../index.js';
// Through the package's own name: the `exports` map's `types`.
import viaExports from 'resvg-napi/helpers';
import { fitTextWidths as viaShim } from 'resvg-napi/fit.mjs';

// Type-checked, never run: `node --test` runs .mts files too, and these calls
// are shapes, not working arguments (a font file that does not exist).
export function compileOnly(): void {
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>';
  const set: Helpers = createHelpers(binding);
  const same: Helpers = helpers;
  const max: number = MAX_PIXELS;

  const parsed: Parsed = parse(svg, {
    fonts: [new Uint8Array(), 'font.ttf'], images: { a: new Uint8Array() }, resourcesDir: '.',
    resourcesRoot: '..', fontFamily: 'x', languages: ['fr'], dpi: 96,
  });
  const tree: binding.usvg.Tree = parsed.tree;
  const pending: string[] = [...parsed.pendingImages, ...parsed.pendingFonts];
  const later: Promise<Parsed> = parseAsync(new Uint8Array(), { fonts: systemFonts(), signal: AbortSignal.timeout(1) });

  const options: RenderOptions = { scale: 2, width: 3, height: 4, background: 'teal', maxPixels: MAX_PIXELS,
    crop: { x: 0, y: 0, width: 1, height: 1 }, fonts: new binding.fontdb.Database() };
  const png: Buffer = renderPng(svg, options);
  const png2: Buffer = renderPng(tree, { crop: binding.tiny_skia.Rect.fromXywh(0, 0, 1, 1)! });
  const raw: RawImage = renderRaw(tree);
  const rawAsync: Promise<RawImage> = renderRawAsync(svg, { signal: new AbortController().signal });
  const pngAsync: Promise<Buffer> = renderPngAsync(tree, { scale: 1 });

  const node = tree.nodeById('n');
  if (node) {
    const one: Buffer = renderNodePng(node, { scale: 2, background: '#fff', maxPixels: 10 });
    const two: Promise<Buffer> = renderNodePngAsync(node, { width: 3, signal: AbortSignal.abort() });
    // @ts-expect-error a node is its own crop
    renderNodePng(node, { crop: { x: 0, y: 0, width: 1, height: 1 } });
    const box = nodeExtent(node);
    console.log(one, two, box?.width);
  }

  const width: number = measureText('hi', { fontSize: 12, family: 'x', fonts: systemFonts() });
  const fitted: FitResult = fitTextWidths(svg, { fonts: [] });
  const fitted2: FitResult = fitTextWidths(svg, (s) => binding.usvg.Tree.fromStr(s));
  const fitted3: FitResult = browser.fitTextWidths(svg, browser.guardedParse(binding));
  // @ts-expect-error the binding-free one needs a render
  browser.fitTextWidths(svg, { fonts: [] });
  const db: Promise<binding.fontdb.Database> = systemFontsAsync();

  // @ts-expect-error no default export in the browser module
  console.log(browser.default);

  const _viaExports: Helpers = viaExports;
  console.log(_viaExports, viaShim);
  console.log(set, same, max, pending, later, png, png2, raw, rawAsync, pngAsync, width, fitted, fitted2, fitted3, db);
}
