// `resvg-napi/helpers` in Node: the helpers of `helpers.browser.mjs`, bound to
// this package's binding (`./index.js`, native or the WASI fallback), as the
// default export and as named exports. A bundler's `browser` condition picks
// `helpers.browser.mjs` instead, which loads no binding: there,
// `createHelpers(binding)` takes the wasm module the page loaded.
import binding from './index.js';
import { createHelpers } from './helpers.browser.mjs';

export * from './helpers.browser.mjs';

const helpers = createHelpers(binding);
export default helpers;

// `fitTextWidths` here is the set's: it takes parse options as well as a
// `render`, so it shadows the binding-free one `export *` brings.
export const {
  parse,
  parseAsync,
  renderPng,
  renderPngAsync,
  renderRaw,
  renderRawAsync,
  renderNodePng,
  renderNodePngAsync,
  measureText,
  fitTextWidths,
  systemFonts,
  systemFontsAsync,
} = helpers;
