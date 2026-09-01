# resvg-napi

[![npm](https://img.shields.io/npm/v/resvg-napi)](https://www.npmjs.com/package/resvg-napi)
[![conformance](https://img.shields.io/badge/resvg%20conformance-1715%2F1715-brightgreen)](#conformance)
[![licence](https://img.shields.io/npm/l/resvg-napi)](#licence)

Node.js bindings for [resvg](https://github.com/linebender/resvg) 0.48: render
SVG to PNG, and read back what usvg made of the document — the resolved tree,
text metrics, paint, geometry.

Generated from the upstream Rust sources, so the TypeScript surface is usvg's
own, names and doc comments included.

```bash
npm install resvg-napi
```

```js
import { Resvg, FontDatabase, renderAsync } from 'resvg-napi'

const doc = new Resvg(svg, { dpi: 192, fontFamily: 'DejaVu Sans' })
doc.renderPng({ width: 1200, background: '#fff' })      // Buffer
doc.renderRaw({ crop: doc.absLayerBoundingBox() })      // RGBA8, trimmed
await renderAsync(svg, options, { width: 1200 })        // off the event loop
```

## What you can read back

`index.d.ts` is the reference — it is generated, so it cannot drift. This is
the map.

**Render.** Parse once with `new Resvg(svg, options?, fonts?, images?)`, then
render as often as you like.

| | |
|---|---|
| `renderPng(params?)` | `Buffer` |
| `renderRaw(params?)` | `RawImage`, un-premultiplied RGBA8 |
| `toString(opts?)` | the resolved tree, back as SVG |
| `renderPngAsync` · `renderRawAsync` · `toStringAsync` | the same, off the event loop, `AbortSignal`-aware |

`params` carries `width` / `height` / `scale`, a `background` colour and a
`crop` box. `Resvg.parseAsync` moves the parse off the loop too.

**Walk the tree.** `node(id)` and `children()` hand back `SvgNode`s, and each
one knows what it is:

```js
const n = doc.node('surname')
n.kind           // 'group' | 'path' | 'image' | 'text'
n.path()         // geometry, fill, stroke — null unless it is a shape
n.text()         // chunks, layouted spans, positioned glyphs, decoration
n.renderPng()    // that element alone, cropped to its own extent
```

A span carries what it is drawn with, down to the lines through it:

```js
const span = n.text().chunks[0].spans[0]
span.decoration.underline?.fill?.paint  // …and .overline, .lineThrough
```

Paint is a discriminated union, so TypeScript narrows it:

```ts
const paint = n.path()?.fill?.paint
if (paint?.type === 'color') paint.value   // { red, green, blue }
else paint?.id                             // the gradient or pattern it names
```

**Measure.** Every node reports `boundingBox()`, `absBoundingBox()`,
`strokeBoundingBox()`, `absLayerBoundingBox()` and `extent()` — in the
document's own units, which is not the canvas. See *Fitting text* below for why
that distinction bites.

**Fonts.** `FontDatabase` is `fontdb` itself: `loadSystemFonts()`,
`loadFontData(buffer)`, `loadFontFile(path)`, `faces()`, `query()`, and the
generic-family setters. A face reports its `families`, `weight`, `style`
(`'normal' | 'italic' | 'oblique'`) and whether it is `monospaced`. `pendingFonts()` on a parsed document names the
families it wanted and did not get; `pendingImages()` does the same for hrefs.

**Definitions.** `linearGradients()`, `radialGradients()`, `patterns()`,
`clipPaths()`, `masks()`, `filters()` — the paint servers an element's `id`
refers to.

Something missing? The generator keeps a report of every upstream member it
left alone, with the reason. `RESVG_NAPI_CODEGEN_LOG=1 cargo build` prints it,
and [CONTRIBUTING.md](CONTRIBUTING.md) explains what the reasons mean.

## Conformance

resvg's own test corpus, rendered through these bindings: **1715 of 1715 match**
the reference PNGs upstream asserts on, within 1/255 per channel — a tolerance
measured rather than chosen, with the reason recorded in
[`scripts/conformance.mjs`](scripts/conformance.mjs).

```bash
npm run conformance:fetch   # the corpus, at the tag Cargo.toml pins
npm run conformance         # 15 seconds including the fetch
```

CI runs it on every pull request, so a resvg bump that changes a render shows up
as a diff rather than passing unnoticed.

## Platforms

Thirteen targets.

| | x64 | arm64 | armv7 | ia32 |
|---|---|---|---|---|
| Linux (glibc) | ✅ | ✅ | ✅ | — |
| Linux (musl) | ✅ | ✅ | — | — |
| macOS | ✅ | ✅ | — | — |
| Windows (MSVC) | ✅ | ✅ | — | ✅ |
| Android | — | ✅ | ✅ | — |
| WASI (`wasm32-wasip1-threads`) | ✅ | — | — | — |

The dependency tree is pure Rust — `fontconfig` support comes from
`fontconfig-parser`, not the C library — so every target cross-compiles without
a system SDK.

The WASI build is the same generated bindings on emnapi instead of node-api, and
it is complete: rendering, `toString`, bounding boxes, the node walk, `renderAsync`
(threads via `wasm32-wasip1-threads`) and even filesystem image resolution all
work. One difference: `loadSystemFonts()` finds 0 faces inside the sandbox, so
fonts must be supplied with `loadFontData(buffer)` — the demo fetches them from
google/fonts by name rather than making you find a file.

Adding a target is an entry in `scripts/ci-targets.mjs` — which host builds it
and which cross-compilation flag it needs — and the triple in `package.json`
under `napi.targets`; then `npm run create-npm-dirs`. The two lists used to be
independent, so a target could sit in one and not the other; the script now
fails when they disagree.

CI does not build all thirteen on a pull request. It builds the two that can
tell it something — `x86_64-unknown-linux-gnu`, which the tests, the drift
checks and the conformance suite run on, and `wasm32-wasip1-threads`, whose
generated shims and test run are a different code path. The other eleven build
on the merge, on a `v*` tag, on a manual dispatch, or on a pull request labelled
`full-matrix` when the proof is wanted before merging rather than after.


## Fitting text to a width

An Illustrator template says how wide a field may be on the element itself
(`data-maxwidth="85"`), which is not SVG — resvg drops it. `fit.mjs` reads the
constraint from the source, measures the rendered text with
`node(id).extent()`, and compresses the element horizontally, composing a
`scale(k 1)` onto whatever transform it already has (the same device those
templates use by hand):

```js
import { fitTextWidths } from 'resvg-napi/fit.mjs'

const { svg, adjustments, problems } = fitTextWidths(source,
  (s) => new Resvg(s, { fontFamily }, fonts))
// adjustments: [{ id: 'surname', from: 200.23, to: 90, factor: 0.4495, measured: 90.0031 }]
```

The limit is read in **the document's own units**, the ones the attribute is
written in. That matters: `width="85.6mm"` with a `viewBox` 240.94 wide makes usvg
normalise the tree to 323.53 units, so `extent()` comes back 1.343× larger than
anything the file says. Comparing raw canvas widths compresses text that actually
fits — everything past `limit / 1.343`.

No `id` needed: an element that has none gets one generated, numbered in document
order, so the constraint does not depend on the template carrying ids. The scale
is anchored at the element's own `x` — a bare `scale()` is anchored at the origin
and slides the text left as it narrows.

A geometric scale is exactly linear in width, so one pass lands on the target;
the second measurement is a check, and it is reported. Text that already fits is
left untouched, and a constraint that cannot be honoured says why: no `id` to
measure by, no visible extent, or an unusable width. It compresses glyphs rather
than reducing `font-size` — the choice the templates already made.

## Diagnostics

usvg and resvg report every recoverable problem through the `log` crate —
unparsable values, skipped shapes, unmatched font families, images that could not
be read. Nothing consumes that by default, so those messages used to vanish:

```js
import { setLogLevel, takeLogs, Resvg } from 'resvg-napi'

setLogLevel('warn')            // off | error | warn | info | debug | trace
new Resvg(svg).renderPng()
takeLogs()
// [ "WARN usvg::parser::style: Failed to parse fill value: 'notacolour'. Fallback to black.",
//   "WARN usvg::parser::shapes: Rect '' has an invalid 'width' value. Skipped." ]
```

`takeLogs()` drains the buffer, which is capped at 500 entries so a pathological
document cannot grow it without end.

## Elsewhere

- **[demo/](demo/README.md)** — a browser proof bench: Liquid templating, an
  array editor, per-element export, live diagnostics. `npm run demo`.
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — how the bindings are generated, and
  what to read before touching the generator.

## Licence

Apache-2.0 OR MIT, matching resvg.
