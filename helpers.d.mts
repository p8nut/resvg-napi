/**
 * `resvg-napi/helpers` in Node: the binding-free module, plus the helper set
 * bound to this package's binding, as the default export and as named
 * exports. Declarations for `helpers.mjs`, written by hand like
 * `helpers.browser.d.mts`.
 */

import type { Helpers } from './helpers.browser.mjs'

export * from './helpers.browser.mjs'

/** The helpers on this package's binding (native, or the WASI fallback). */
declare const helpers: Helpers
export default helpers

export declare const parse: Helpers['parse']
export declare const parseAsync: Helpers['parseAsync']
export declare const renderPng: Helpers['renderPng']
export declare const renderPngAsync: Helpers['renderPngAsync']
export declare const renderRaw: Helpers['renderRaw']
export declare const renderRawAsync: Helpers['renderRawAsync']
export declare const renderNodePng: Helpers['renderNodePng']
export declare const renderNodePngAsync: Helpers['renderNodePngAsync']
export declare const measureText: Helpers['measureText']
/** The set's: takes parse options, or a `render` as the binding-free one does. */
export declare const fitTextWidths: Helpers['fitTextWidths']
export declare const systemFonts: Helpers['systemFonts']
export declare const systemFontsAsync: Helpers['systemFontsAsync']
