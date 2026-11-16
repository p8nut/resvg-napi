/**
 * `resvg-napi/helpers` in Node: the binding-free module of
 * `helpers.browser.mjs`, plus the helper set bound to this package's binding
 * (`./index.js`, native or the WASI fallback), as the default export and as
 * named exports. A bundler's `browser` condition picks `helpers.browser.mjs`
 * instead, which loads no binding: there, `createHelpers(binding)` takes the
 * wasm module the page loaded.
 *
 * `helpers.mjs` and `helpers.d.mts` are emitted by `tsc` from `helpers.mts`,
 * like `helpers.browser.mjs` and `helpers.browser.d.mts`.
 */

import binding from './index.js';
import { createHelpers } from './helpers.browser.mjs';
import type { Helpers } from './helpers.browser.mjs';

export * from './helpers.browser.mjs';

/** The helpers on this package's binding (native, or the WASI fallback). */
const helpers: Helpers = createHelpers(binding);
export default helpers;

// `fitTextWidths` here is the set's: it takes parse options as well as a
// `render`, so it shadows the binding-free one `export *` brings.
export const parse: Helpers['parse'] = helpers.parse;
export const parseAsync: Helpers['parseAsync'] = helpers.parseAsync;
export const renderPng: Helpers['renderPng'] = helpers.renderPng;
export const renderPngAsync: Helpers['renderPngAsync'] = helpers.renderPngAsync;
export const renderRaw: Helpers['renderRaw'] = helpers.renderRaw;
export const renderRawAsync: Helpers['renderRawAsync'] = helpers.renderRawAsync;
export const renderNodePng: Helpers['renderNodePng'] = helpers.renderNodePng;
export const renderNodePngAsync: Helpers['renderNodePngAsync'] = helpers.renderNodePngAsync;
export const measureText: Helpers['measureText'] = helpers.measureText;
/** The set's: takes parse options, or a `render` as the binding-free one does. */
export const fitTextWidths: Helpers['fitTextWidths'] = helpers.fitTextWidths;
export const systemFonts: Helpers['systemFonts'] = helpers.systemFonts;
export const systemFontsAsync: Helpers['systemFontsAsync'] = helpers.systemFontsAsync;
