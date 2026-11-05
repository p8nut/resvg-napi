/**
 * The helpers, without a binding: `createHelpers(binding)` builds them on
 * the binding it is given. What a bundler's `browser` condition resolves
 * `resvg-napi/helpers` to; in Node, `helpers.d.mts` adds the set bound to the
 * native binding.
 *
 * Declarations for `helpers.browser.mjs`, which is JavaScript: written by
 * hand, unlike `index.d.ts`, so they can drift. `npm run typecheck` reads
 * them against `test/helpers-types.mts`.
 */

import type { usvg, fontdb, guard, resvg, svgtypes, tiny_skia, BBox } from './index.js'

/** What `createHelpers` needs of a binding: its namespaces. */
export interface Binding {
  usvg: typeof usvg
  fontdb: typeof fontdb
  guard: typeof guard
  resvg: typeof resvg
  svgtypes: typeof svgtypes
  tiny_skia: typeof tiny_skia
}

/** A `usvg.Node`: what `tree.nodeById` and `group.children()` hand back. */
export type Node = usvg.NodeGroup | usvg.NodePath | usvg.NodeImage | usvg.NodeText

/** The canvas cap the helpers default to: 2^26 px, an 8192x8192 canvas or 256 MiB. */
export declare const MAX_PIXELS: number

/**
 * How the helpers parse: `usvg.Options`, with the fonts and the guard's
 * options beside them. Every parse goes through a fresh
 * `guard.ImageHrefResolver` and `guard.FontResolver` unless one is passed.
 */
export interface ParseOptions extends Omit<usvg.Options, 'fontdb'> {
  /**
   * The fonts: a `fontdb.Database`, or font files (paths) and buffers to load
   * into a new one. Default: the system fonts, scanned once per binding and
   * shared (`systemFonts()`). Under WASI the system has none.
   */
  fonts?: fontdb.Database | Array<string | Uint8Array>
  /** usvg's name for `fonts`, when it is a database. `fonts` wins. */
  fontdb?: fontdb.Database
  /** Image bytes by href, tried before any file (`guard.ImageHrefResolverOptions`). */
  images?: Record<string, Uint8Array>
  /**
   * Widens what hrefs may read to this ancestor of `resourcesDir`. Files are
   * read only under `resourcesDir`; without it, none.
   */
  resourcesRoot?: string
}

/** `parseAsync`'s options: a signal drops the parse while it is queued. */
export interface ParseAsyncOptions extends ParseOptions {
  signal?: AbortSignal
}

/** A parsed document and what it could not resolve. */
export interface Parsed {
  tree: usvg.Tree
  /** The `<image>` hrefs nothing resolved, or that were refused. */
  pendingImages: string[]
  /** The named font families the fonts lack: text in them fell back. */
  pendingFonts: string[]
}

/** The size and look of one render. */
export interface RenderParams {
  /** Uniform scale factor. Default: 1. */
  scale?: number
  /** Target width in px, rounded; keeps the aspect ratio, overrides `scale`. */
  width?: number
  /** Target height in px, rounded; keeps the aspect ratio, overrides `scale` (`width` wins). */
  height?: number
  /**
   * Background: any CSS colour `svgtypes.Color.fromStr` reads (`#eee`,
   * `teal`, `rgba(255, 0, 0, .5)`). Default: transparent.
   */
  background?: string
  /**
   * Render only this rectangle, in the tree's units: sizing then applies to
   * the crop. A `BBox` (`nodeExtent(node)`) or a `tiny_skia.Rect`.
   */
  crop?: BBox | tiny_skia.Rect
  /**
   * Refuse a canvas of more than this many pixels, before allocating it.
   * Default: `MAX_PIXELS`. A group with a filter, opacity, mask or clip path
   * renders through a layer of its own, which can be larger.
   */
  maxPixels?: number
}

/** A render that may parse: parse options and render params in one object. */
export interface RenderOptions extends ParseOptions, RenderParams {}

/** `RenderOptions` for the async twins: a signal drops a queued step. */
export interface RenderAsyncOptions extends RenderOptions {
  signal?: AbortSignal
}

/** A node render's params: no `crop`, the node is the crop. */
export type NodeRenderParams = Omit<RenderParams, 'crop'>

/** Un-premultiplied RGBA8 pixels, row-major, no padding. */
export interface RawImage {
  width: number
  height: number
  data: Buffer
}

/** `measureText`'s options: the text's size and family, and how to parse. */
export interface MeasureOptions extends Omit<ParseOptions, 'fontSize'> {
  /** Font size of the text. Default: 12. */
  fontSize?: number
  /** Its family. Default: `fontFamily`, else `sans-serif`. */
  family?: string
}

/** What a document's source is: SVG text, or its bytes (not SVGZ). */
export type Source = string | Uint8Array

/** The helper set `createHelpers` builds. */
export interface Helpers {
  /** Parses through the guard. Throws the binding's parse errors; SVGZ with advice. */
  parse(svg: Source, options?: ParseOptions): Parsed
  /** `parse` on a worker thread; the system fonts, if used, are scanned on one too. */
  parseAsync(svg: Source, options?: ParseAsyncOptions): Promise<Parsed>
  /**
   * A PNG of `svg` (parsed with `options`, then freed) or of a parsed tree
   * (left as it is, parse options ignored).
   */
  renderPng(svg: Source | usvg.Tree, options?: RenderOptions): Buffer
  /** `renderPng` off the main thread: parse, render and encode each take a worker trip. */
  renderPngAsync(svg: Source | usvg.Tree, options?: RenderAsyncOptions): Promise<Buffer>
  /** `renderPng`, as raw pixels. */
  renderRaw(svg: Source | usvg.Tree, options?: RenderOptions): RawImage
  /** `renderRaw` off the main thread. */
  renderRawAsync(svg: Source | usvg.Tree, options?: RenderAsyncOptions): Promise<RawImage>
  /** One node in a PNG sized to its extent (`nodeExtent`). */
  renderNodePng(node: Node, params?: NodeRenderParams): Buffer
  /** `renderNodePng` off the main thread. */
  renderNodePngAsync(node: Node, params?: NodeRenderParams & { signal?: AbortSignal }): Promise<Buffer>
  /** The width `text` draws at, in user units: its stroke box. 0 when nothing draws. */
  measureText(text: string, options?: MeasureOptions): number
  /**
   * `fitTextWidths`, parsing each pass with `parse(svg, options)`, or with a
   * `render` of your own.
   */
  fitTextWidths(svg: string, options?: ParseOptions | ((svg: string) => Renderer)): FitResult
  /** The shared system fonts, scanned on the first call. */
  systemFonts(): fontdb.Database
  /** The shared system fonts, scanned on a worker thread. A signal abandons the wait. */
  systemFontsAsync(signal?: AbortSignal): Promise<fontdb.Database>
}

/**
 * The helpers on `binding`: in a browser, the wasm module the page loaded.
 * One set per binding, so the system fonts are scanned once. Throws a
 * `TypeError` when `binding` is not a loaded resvg-napi module.
 */
export function createHelpers(binding: Binding): Helpers

/** What fitting reads of a parsed `usvg.Tree`. */
export type Renderer = Pick<usvg.Tree, 'size' | 'nodeById'>

/** One element carrying a width constraint, and what is needed to rewrite it. */
export interface WidthConstraint {
  /** The element's opening tag, verbatim. */
  tag: string
  /** The tag name: `text`, `tspan`, ... */
  name: string
  /** The attribute text inside that tag, verbatim. */
  attrs: string
  /** The element's `id`, or null when it had none and one must be generated. */
  id: string | null
  /** The limit, in the document's own units -- not canvas pixels. */
  max: number
  /** Byte offset of the tag in the source string. */
  index: number
}

/** One element that was compressed, and by how much. */
export interface Adjustment {
  id: string
  /** Measured width before, in the document's units. */
  from: number
  /** The limit it was fitted to. */
  to: number
  /** The horizontal scale applied: `to / from`. */
  factor: number
  /** Measured width after, as a check on the one-pass assumption. */
  measured: number
}

export interface FitResult {
  /** The source, with a `scale(k 1)` composed onto each fitted element. */
  svg: string
  adjustments: Adjustment[]
  /** Constraints that could not be honoured, each saying why. */
  problems: string[]
}

/**
 * Where a node will draw, contours and filters included: a group's layer box,
 * a path's or a text's stroke box, an image's box. `null` when it has no
 * visible size.
 */
export function nodeExtent(node: Node): BBox | null

/** Elements carrying a width constraint, with the tag text needed to rewrite it. */
export function findWidthConstraints(svg: string): WidthConstraint[]

/** What `guardedParse` passes on: `usvg.Options`, with the resolver's own. */
export type GuardedParseOptions = Omit<usvg.Options, 'imageHrefResolver'> &
  Pick<guard.ImageHrefResolverOptions, 'images' | 'resourcesRoot'>

/**
 * A `render` for `fitTextWidths` that is safe on untrusted SVG: each parse
 * gets a fresh `guard.ImageHrefResolver` (images supplied by href, files only
 * under `resourcesDir`, SVGZ and oversized images refused). The raw
 * `usvg.Tree.fromStr` resolves `<image href>` from any local path instead.
 */
export function guardedParse(
  binding: { usvg: typeof usvg; guard: typeof guard },
  options?: GuardedParseOptions,
): (svg: string) => usvg.Tree

/**
 * Fits every `data-maxwidth` element by compressing it horizontally.
 *
 * @param svg     source, after any templating
 * @param render  `(svg) => usvg.Tree`; supply fonts and options here.
 *                 `guardedParse(binding, options)` is the safe default. The
 *                 raw `usvg.Tree.fromStr` resolves `<image href>` from any
 *                 local path: do not pass untrusted SVG through it without a
 *                 `guard.ImageHrefResolver`.
 */
export function fitTextWidths(
  svg: string,
  render: (svg: string) => Renderer,
): FitResult
