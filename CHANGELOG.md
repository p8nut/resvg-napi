# Changelog

## 0.2.0 — 2026-09-01

Three things you could not read before, and nineteen defects an adversarial
review found. The minor bump is for the API additions; two of the fixes change
shapes, and they are listed under Breaking.

### New

`SvgNode.image()` returns the content of an image node, and the four raster
variants of its `kind` carry the encoded bytes exactly as the document supplied
them. usvg says a raster payload should be decoded by the caller; this is the
caller.

```js
const img = node.image()
img.kind.type    // 'png' | 'jpeg' | 'gif' | 'webp' | 'svg'
img.kind.bytes   // Buffer
```

`TextSpan.decoration` gives `underline`, `overline` and `lineThrough`, each with
the fill and stroke it is drawn with. `FontFace.style` says
`'normal' | 'italic' | 'oblique'`.

### Breaking

The string enums are type-only unions rather than ambient `const enum`s. An
ambient `const enum` cannot be used under `isolatedModules` -- every bundler --
so `shapeRendering`, `textRendering` and `imageRendering` had no way to be set at
all. Write `shapeRendering: 'geometricPrecision'` instead of
`ShapeRendering.GeometricPrecision`; the runtime is unchanged.

`Span` and `PositionedGlyph` are read-only classes rather than plain objects.
They expose more than before -- `PositionedGlyph` gained `id` and `text` -- and
the object form only existed because those members were invisible.

### Fixed, and each of these shipped

**A requested width came back one pixel too wide.** Seventeen of the first four
hundred widths were wrong on a 100x50 document; `renderPng({width: 120})`
produced a PNG whose IHDR read 121x61. The scale was an f32 round trip, and every
width in the test suite was an exact multiple, so nothing could see it.

**Three fields declared `string` while holding a `Path`.** napi maps a type
*named* `Path` to `string` from the name alone. `span.underline.fill` was a type
error on working code.

**`npm ci` was impossible** -- the lock file predated the rename.

**A conformance case that started throwing was reported as an improvement.**

**The `Matrix` doc claimed SVG's `matrix()` field order.** It is `sx ky kx sy tx
ty`, and reading the fields positionally mirrors the transform.

`browser.js` now declares the wasm package it imports, `fit.mjs` ships type
declarations, `setLogLevel` declares its six levels, and the publish loop can be
retried.

One thing about `npm ci` worth knowing rather than filing: it works against a
published version and cannot work against an unpublished one. The manifest names
its platform packages at the version being released, and until that version is on
the registry `npm install` cannot resolve them, so the lock file has nothing to
record and `npm ci` calls them missing. That is why CI uses `npm install`.

### Guarded

One new upstream accessor used to delete every public field of a data type,
silently. The POW_VEC precision clamp was keyed on a field-name suffix with no
floor -- a rename upstream meant an index 242 past a 13-entry table, aborting the
process. Handle discovery truncated at 24 while the report listed the classes it
had dropped as generated. All three fail the build now, and each was made to fail
before being trusted.

### Known

`defaultSizeWidth` and `defaultSizeHeight` reach usvg and usvg does not honour
them: its converter rewrites the size from `default_size` and then recomputes it
from the attribute, ignoring the rewrite. `test/options.mjs` records both the
behaviour and the reason.

## 0.1.2 — 2026-08-31

Same contents as 0.1.1, which never fully published. The number moved because
npm burned it on one of the fourteen packages.

What happened, because it is worth knowing before unpublishing anything on npm:
0.1.0 was unpublished by mistake, and npm blocks republishing a name for 24
hours after that -- but the block outlives the clock. More than a day later the
registry still refused to save a new packument for those names, answering

    409 Conflict — Failed to save packument. A common cause is if you try to
    publish a new package before the previous package has been fully processed.

The publish job died on the first of the fourteen. Except the registry then
accepted that first PUT anyway, forty minutes later, while having told the
client it had failed -- so `resvg-napi-android-arm-eabi@0.1.1` exists and 0.1.1
is spent there. A version set where thirteen packages are 0.1.1 and one cannot
be is worse than a skipped number.

Nothing was installable at any point: the root package publishes last, so a
failure part-way leaves platform packages that nothing references rather than a
root package pointing at binaries that are not there. That ordering was put in
for exactly this and is the reason this entry describes an inconvenience instead
of a broken release.

## 0.1.1 — 2026-08-31 (never published)

The first installable release. 0.1.0 was published and then unpublished by
mistake, and npm never lets a `name@version` be reused, so the number had to
move. The wait for the name to unblock was spent on the generator rather than
idle, so this is not an empty bump.

### Text decoration

`TextSpan.decoration` hands back `underline`, `overline` and `lineThrough`, each
with the `fill` and `stroke` it is drawn with. Nothing could read them before.

```js
const span = doc.node('title').text().chunks[0].spans[0];
span.decoration.underline?.fill.paint   // { type: 'color', value: {…} }
```

### Font style

`FontFace.style` says `'normal' | 'italic' | 'oblique'` instead of not existing.

### What made those possible

Both were generator faults, and neither was specific to the member it hid.

The enum pass only ever read usvg's sources, so nothing fontdb defines could be
mapped -- and it read a crate's private modules, where fontdb vendors a copy of
ttf-parser, emitting one type twice and naming another that does not exist. It
also only wanted enums that some method returned, which missed every enum used
as a plain struct field.

And the object fixpoint judged a type unmappable before letting it queue the
types it needed, so anything whose members were all still undiscovered was
written off on its first turn and took its dependencies down with it. That is
what `TextDecoration` was: it reaches only `TextDecorationStyle`.

### Not done, and why

Image bytes. `ImageKind` carries the encoded bytes of an embedded image, and
reaching them needs the enum emitted as a union -- but a union variant is an
`#[napi(object)]`, and napi requires `Clone` of every field. `Buffer` is a
handle into the JS heap and has none. The codegen report says so now, in place
of the wrong reason it used to give.

### Enforced after the fact

`npm version` bumps the root manifest and the thirteen platform manifests but
not the `optionalDependencies` naming them. Left as it landed, 0.1.1 would have
asked for platform packages at 0.1.0 -- a version that can never exist again --
and `npm install` would have resolved nothing. `check:package` refuses any
version drift between the root, its pins and the platform manifests.

Also worth knowing: `npm unpublish` prints `- <name>` whether or not the
registry accepted it. The `DELETE` behind it can return 404 -- an expired local
credential is enough -- and the CLI reports success anyway.

## 0.1.0 — 2026-08-30 (unpublished)

First release. There is no upgrade path to describe; this entry is what the
package contains.

`resvg-napi` and its `resvg-napi-<platform>` names were unclaimed on npm while
this manifest already declared them as optional dependencies -- so the first
person to publish one of them would have had their code resolved by
`npm install` here, and in CI. Publishing all of them closes that: they are
occupied now.

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

### Conformance

resvg's own test corpus, rendered through these bindings: **1715 of 1715 match**
the reference PNGs upstream asserts on, within 1/255 per channel -- a tolerance
measured rather than chosen, and the reason for it is in `scripts/conformance.mjs`.
`npm run conformance` reproduces it, and CI runs it on every pull request.

### Platforms

Twelve native targets plus `wasm32-wasip1-threads`: Linux glibc (x64, arm64,
armv7), Linux musl (x64, arm64), macOS (x64, arm64), Windows MSVC (x64, ia32,
arm64) and Android (arm64, armv7).

The test suite runs against both the native binding and the wasm one
(`npm run test:wasi`); the wasm build has no system fonts, which is why nothing
in the suite assumes a font is installed.

### Demo

`demo/` is a bench for the whole surface: Liquid templating with static
analysis, partials, filters that measure text through resvg, the resolved usvg
tree, a node list with per-element PNG export, and four worked examples that
also render headlessly through `demo/render.mjs`.

Fonts are fetched by name rather than found on disk: a family from
google/fonts, `fontawesome`, or any URL to a `.ttf`. The Google Fonts CSS API
is no use for this -- it serves a browser WOFF2, which `ttf-parser` does not
read -- so the original TTFs come from the google/fonts repository.
