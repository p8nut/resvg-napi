/**
 * Horizontal text fitting, driven by `data-maxwidth`.
 *
 * Declarations for `fit.mjs`, which is JavaScript with JSDoc: written by hand,
 * unlike `index.d.ts`, so they can drift. `npm run typecheck` reads them.
 */

/** A `Resvg` instance, however the caller chose to build one. */
type Renderer = {
  node(id: string): { extent(): { width: number } | null } | null
}

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

/** Elements carrying a width constraint, with the tag text needed to rewrite it. */
export function findWidthConstraints(svg: string): WidthConstraint[]

/**
 * Fits every `data-maxwidth` element by compressing it horizontally.
 *
 * @param svg     source, after any templating
 * @param render  `(svg) => Resvg`; supply fonts and options here
 */
export function fitTextWidths(
  svg: string,
  render: (svg: string) => Renderer,
): FitResult
