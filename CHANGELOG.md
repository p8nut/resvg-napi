# Changelog

## 0.1.0 — unreleased

First cut. Nothing has been published yet, so there is no upgrade path to
describe; this entry is what the package contains.

### The bindings are generated

`build.rs` parses the pinned `usvg`, `resvg`, `tiny-skia` and `fontdb` sources
with `syn` and emits `src/lib.rs` with `quote`. Nothing that describes upstream
is written by hand: type aliases, newtypes, `Deref` chains, enum variants and
struct fields are all read from the sources, and the API decisions live in one
`quote!` template. `src/lib.rs` is committed and CI fails on any diff, so a
change in the pinned upstream shows up as a build product change.

Against resvg 0.48.1 / usvg 0.48.1 / fontdb 0.24 / tiny-skia 0.12.

### Surface

- `Resvg`: `renderPng`, `renderRaw`, `renderNodePng`, `toString`, `width`,
  `height`, `hasTextNodes`, `pendingFonts`, `pendingImages`, `resolveImage`,
  the bounding-box family, `querySelector`/`getElementsByTagName`, `node`,
  `children`, and the definition tables (`linearGradients`, `radialGradients`,
  `patterns`, `filters`, `masks`, `clipPaths`, `fontdb`).
- `renderAsync(svg, options, params, signal)` on the libuv pool, cancellable
  through an `AbortSignal`.
- `SvgNode`, `FontDatabase`, `FontFace`, and the definition classes as opaque
  `#[napi]` classes; every value object as an `#[napi(object)]` interface; 24
  string enums.
- `fit.mjs`: horizontal text fitting driven by `data-maxwidth`, measured with
  the renderer rather than estimated.
- Diagnostics: `setLogLevel` / `takeLogs` collect what usvg and resvg report.

### Platforms

Eight native targets plus `wasm32-wasip1-threads`. The test suite runs against
both the native binding and the wasm one (`npm run test:wasi`); the wasm build
has no system fonts, which is why nothing in the suite assumes a font is
installed.

### Demo

`demo/` is a bench for the whole surface: Liquid templating with static
analysis, partials, filters that measure text through resvg, the resolved usvg
tree, a node list with per-element PNG export, and four worked examples that
also render headlessly through `demo/render.mjs`.
