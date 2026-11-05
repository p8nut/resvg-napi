// Moved to the helpers (`resvg-napi/helpers`, step 11 of
// docs/migration-0.10.md). This shim keeps `resvg-napi/fit.mjs` working until
// the old API goes (step 13). It re-exports the binding-free module, so it
// loads in a browser bundle as before.
export { findWidthConstraints, fitTextWidths, guardedParse, nodeExtent } from './helpers.browser.mjs';
