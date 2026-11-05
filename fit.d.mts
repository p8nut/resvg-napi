/**
 * Horizontal text fitting, driven by `data-maxwidth`.
 *
 * Declarations for `fit.mjs`, which is JavaScript with JSDoc: written by hand,
 * unlike `index.d.ts`, so they can drift. `npm run typecheck` reads them.
 */

import type { usvg, guard, BBox } from './index.js'

/** What fitting reads of a parsed `usvg.Tree`. */
type Renderer = Pick<usvg.Tree, 'size' | 'nodeById'>

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
export function nodeExtent(node: usvg.NodeGroup | usvg.NodePath | usvg.NodeImage | usvg.NodeText): BBox | null

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
