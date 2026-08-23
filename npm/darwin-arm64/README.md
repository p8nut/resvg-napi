# resvg-napi

Node.js bindings for [resvg](https://github.com/linebender/resvg) 0.48, generated
from the upstream Rust sources.

```js
import { Resvg, FontDatabase, renderAsync } from 'resvg-napi'

const doc = new Resvg(svg, { dpi: 192, fontFamily: 'DejaVu Sans' })
doc.renderPng({ width: 1200, background: '#fff' })      // Buffer
doc.renderRaw({ crop: doc.absLayerBoundingBox() })      // RGBA8, trimmed
await renderAsync(svg, options, { width: 1200 })        // off the event loop
```

## How this package is built

`build.rs` parses the sources of `usvg`, `resvg`, `fontdb`, `tiny-skia-path` and
`strict-num` with `syn`, and emits `src/lib.rs` with `quote`. Everything that
describes upstream — types, names, signatures, doc comments, even bounds such as
the precision clamp read out of usvg's `POW_VEC` — is derived. The hand-written
part is the API shape: what a `Pixmap` becomes, how images and fonts are
resolved, what runs on a worker thread.

`src/lib.rs`, `index.js` and `index.d.ts` are generated *and* committed, and the
generator is deterministic: CI regenerates them and fails on any diff.

To build against a checkout instead of the crates.io cache:

```bash
USVG_SRC_DIR=/path/to/resvg/crates/usvg/src cargo build --release
```

## Platforms

| | x64 | arm64 |
|---|---|---|
| Linux (glibc) | ✅ | ✅ |
| Linux (musl) | ✅ | ✅ |
| macOS | ✅ | ✅ |
| Windows (MSVC) | ✅ | ✅ |
| WASI (`wasm32-wasip1-threads`) | ✅ | — |

The dependency tree is pure Rust — `fontconfig` support comes from
`fontconfig-parser`, not the C library — so every target cross-compiles without
a system SDK.

The WASI build is the same generated bindings on emnapi instead of node-api, and
it is complete: rendering, `toString`, bounding boxes, the node walk, `renderAsync`
(threads via `wasm32-wasip1-threads`) and even filesystem image resolution all
work. One difference: `loadSystemFonts()` finds 0 faces inside the sandbox, so
fonts must be supplied with `loadFontData(buffer)`.

Adding a target is one line in `package.json` under `napi.targets`, plus a row in
the CI matrix; then `npm run create-npm-dirs`.

## Browser demo

```bash
npm run demo      # builds the wasm, bundles the loaders, serves on :8787
```

![demo](demo/screenshot.png)

The page parses the SVG first and **asks for what the document is missing**, then
lets you supply it:

- **fonts** — `pendingFonts()` gives the named families the database does not
  have; a text element with no font loaded gets its own prompt (a generic
  `font-family` never appears as a named family).
- **images** — `pendingImages()` gives the hrefs neither the uploads nor the
  filesystem resolved, one file field per href, because the file/href pairing has
  to be explicit.
- **a variable used as an image href** gets a *file* field instead of a text one:
  the page scans the `href` of every `<image>` tag for `{{ … }}` and, once a file
  is picked, substitutes a synthetic `var:<name>` href and registers the bytes
  under it — so resolution never depends on what the placeholder renders to.

Drop files on the zone, paste them, or use the per-item fields; a font goes to
the database, an image is matched to a pending href by file name when it matches
and to the first one still waiting otherwise. Uploads are kept in a page-level
map and handed to every parse, so they survive edits and re-renders — unlike
`resolveImage`, which only patches one instance.

### Liquid templates

The page runs the SVG through [LiquidJS](https://liquidjs.com) first, so a
templated document can be previewed:

- `liquid.globalVariablesSync()` lists the variables the template expects, and the
  page builds one field per variable.
- A field left empty renders back to its own `{{ name }}`, which is why an image
  variable stays visible to `pendingImages()` and can still be uploaded.
- A variable that is piped through filters shows the whole chain
  (`upn | qr: '#ffffff', '#00000000'`) — the static analysis only reports the
  variable name, so the chains are read off the output tags.
- `strictFilters` is off, so an unknown filter passes its value through instead of
  throwing. `qr` is registered as a **stub returning an empty string**; swap it
  for the real QR-code SVG generator in `demo/index.html`.

Typing in a variable field records the value immediately but defers the work:
re-parsing is debounced to 200 ms, re-rendering to 350 ms, and the fields are
only rebuilt when the *set* of variables changes — rebuilding them on every
keystroke destroys the focused input mid-word.

Two input guards, both learned the hard way: a leading newline before
`<?xml … ?>` makes usvg reject the document (Illustrator exports have one), and
an XML declaration pasted into a variable field would land mid-file. Both are
stripped in `source()`.

### Browser specifics

Three things the demo has to handle, all visible in
`demo/index.html`:

- **COOP + COEP headers.** The wasm is built for `wasm32-wasip1-threads`, so it
  wants a shared memory, so it needs `SharedArrayBuffer`, so the page must be
  cross-origin isolated. `demo/serve.mjs` sets both headers.
- **A `Buffer` polyfill.** napi's `Buffer` return type is Node's Buffer and
  emnapi looks it up on `globalThis`. A 6-line `Uint8Array` subclass is enough —
  but its `from` must accept a `SharedArrayBuffer`, or every buffer comes back
  empty.
- **Copy out of shared memory.** `Blob`, `fetch` bodies and `postMessage` all
  refuse views backed by a `SharedArrayBuffer`; `new Uint8Array(buf)` copies.

## Scripts

```
npm run build        # napi build --platform --release
npm test             # 9 test files, native + typings
npm run typecheck    # tsc --strict over index.d.ts and demo.mts
```

## Licence

Apache-2.0 OR MIT, matching resvg.
