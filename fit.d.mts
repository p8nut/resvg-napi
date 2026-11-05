/**
 * Horizontal text fitting, driven by `data-maxwidth`.
 *
 * Moved to `resvg-napi/helpers` (`helpers.browser.d.mts`); this shim's
 * declarations stay with `fit.mjs` until step 13 of docs/migration-0.10.md.
 */

export {
  findWidthConstraints,
  fitTextWidths,
  guardedParse,
  nodeExtent,
} from './helpers.browser.mjs'
export type {
  Adjustment,
  FitResult,
  GuardedParseOptions,
  WidthConstraint,
} from './helpers.browser.mjs'
