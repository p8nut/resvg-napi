# Migration to 0.10.0

This is how [ADR 0001](adr/0001-thin-generic-wrapper.md) and
[ADR 0002](adr/0002-api-details.md) get built. Each step is one pull
request on `main`.

- The old API (the `Resvg` class and the flat exports) and the new one (one
  namespace per crate) coexist until PR 13. Nothing is released before PR 14.
- Every PR keeps CI's drift checks byte-exact on the committed `src/lib.rs`,
  `index.d.ts` and `codegen-report.txt`.
- Tests for the new API go in `test/api/`, and helper tests in
  `test/helpers/`. Both run natively and under WASI.

Tick a step off here when it is merged.

## Steps

| # | Pull request | Main risk |
|---|---|---|
| 1 | Make the generator a crate, `crates/napi-gen`, with its config passed as a value. Output stays byte-identical. | low |
| 2 | Generic boundary: remove the resvg names from `napi-gen` and `napi-support` (pass labels, domain shapes and the node view become config hooks), add the CI grep, and add a toy crate built and tested in CI. | over-parametrised hooks |
| 3 | `index.d.ts` diff classifier and `breaking-api` label, in a `pull_request_target` workflow that never runs PR code. `resvg.yml` applies the label itself. | workflow security |
| 4 | One namespace per crate as a second surface (`usvg.*`, `fontdb.*`). `fontdb.Database` becomes a generic `Arc` class with `make_mut` for `&mut` methods. Rust names are mirrored. | binary size roughly doubles until PR 13 |
| 4b | Flat classes named like a namespaced one are renamed `Legacy*` (`ClipPath` becomes `LegacyClipPath`), by a `Config::legacy_prefix` the generator applies to the flat surface, so the 17 usvg classes join `usvg.*` (decision Q36). The `Legacy*` classes go with the flat surface in step 13. Labelled `breaking-api`. | renames on the flat surface |
| 5 | Option structs as plain objects with `Default`, plus field providers for fields with no JS form. Start with a spike on the `fontdb` field. | `fontdb` field |
| 6 | Factories, free functions and `Result` errors. Owned and consuming classes. `resvg`, `tiny-skia` (allowlist: `Pixmap`, `Color`, `IntSize`, `Transform`, `Rect`, `Size`) and `svgtypes` (`Color`) become sources. `FromStr` gives `fromStr`, `Display` gives `toString`, and plain-data classes get `fromObject`/`toObject`. | allowlist creep |
| 7 | Generic projections over borrowed values: `tree.root()`, `group.children()` returning `usvg.Node` unions, `resvg.renderNode`. | highest: stop and decide if the generic step path passes about 300 lines |
| 7b | Old node surface removed early (Q37 option D): `SvgNode`, `Resvg.node()` / `children()` / `renderNodePng(Async)` / `nodeBbox` / `faceOf` and the definition tables on `Resvg`, the `Legacy*` classes, the flat data types only nodes reached, and the generator hooks that served them (`content_view`, `legacy_prefix`, the template's `Doc`). Labelled `breaking-api`. | tests lose what has no new-API form yet |
| 8 | Generic `dispose()` / `disposed` / `using` for every class, with a generated `.d.ts` header. | low |
| 9 | Async twins listed in config, with leases for `&mut` arguments. | the put-back path on abort |
| 10 | The resvg-specific `guard` module (hand-written, outside the grep): `ImageHrefResolver`, `FontResolver`, `resourcesDir` fence, 256 MiB, 2^28 px, SVGZ refused. **Must also become the default of every helper that parses caller SVG: `fit.mjs` (`fitTextWidths` / `nodeExtent` callers) and the demo's `measureText`.** Since 7b they take a caller-built `usvg.Tree`, and the raw API resolves `<image href>` from any path (Q10); until this step, untrusted SVG must not go through them without a resolver. | one provider per parse; local file read through `fit` if forgotten |
| 11 | `resvg-napi/helpers` in JS: `parse`, `renderPng`/`renderPngAsync`, `renderRaw`, `renderNodePng`, `measureText`, `fit`, system fonts, `maxPixels`, `background` through `svgtypes.Color.fromStr`, the friendlier SVGZ message, `createHelpers(binding)`, and `package.json` `exports`. | `exports` closes deep imports |
| 12 | Demo and conformance move to the helpers and the new API. | conformance baseline moves |
| 13 | Remove the old API. Must come out labelled `breaking-api`. | large: split into 13a (Rust) and 13b (JS, tests, docs) if needed |
| 14 | Release 0.10.0. The notes name what left (#181). | — |

Step 4 found a limit of napi (3.16): it keeps one constructor per JS class
name, whatever the namespace. A namespaced class with the same name as a flat
class would break both, so step 4 left out every usvg class (`ClipPath`,
`Mask`, `Pattern`, the gradients, `Text`, `Image`...), which step 7 needs.
Step 4b settles it (decision Q36): the flat copy gives way and is renamed
`Legacy*`, and `usvg.*` gets the upstream name (CONTRIBUTING, "The namespaced
surface"). `fontdb.Database`, `fontdb.FaceInfo` and `usvg.Tree` had names of
their own already.

Step 5 started with a spike on the `fontdb` field (ADR 0002, Q27): can a
plain options object hold a class instance? It can, under napi 3.16.1 and
napi-derive 3.7.0, natively and under WASI. An input-only
`#[napi(object, object_to_js = false)]` struct takes a field
`Option<ClassInstance<'a, C>>` (the struct gets the lifetime) or
`Option<&'static C>`; the `.d.ts` names the class with its namespace
(`fontdb?: fontdb.Database`). napi checks the instance on the way in: another
class's instance or a plain object throws, by type tag natively and by the
class recorded at wrap time on WASI. A `&'a C` field does not compile: napi-derive
renames only the lifetimes of generic arguments. So `usvg.Options.fontdb` takes
a `fontdb.Database` (as `ClassInstance`), and no trailing argument is needed:
step 6's `usvg.Tree.fromData(data, options)` keeps the Rust signature. The
same mechanism serves the resolvers' provider classes in step 10
(`Config::providers`). Nothing namespaced takes the options yet, and
`fontdb.Database` has no constructor yet, so the root's runtime test of the
field waits for step 6; the toy tests the mechanism (`toy.report`).

Step 5 left out:

- `usvg.Options.imageHrefResolver` and `fontResolver`: no provider until the
  `guard` classes of step 10. usvg's default resolvers apply.
- `defaultSize` stays flattened into `defaultSizeWidth` / `defaultSizeHeight`,
  as on the flat options, until `tiny_skia.Size` exists (step 6).
- `indent` and `attributesIndent` take `number | 'tabs' | 'none'` through a
  converter, not a generated union: `Indent` is xmlwriter's, which the
  generator does not parse, and payload enums are output-only so far.

Step 6 came in two pull requests: 6a, the generator (`napi_gen::class`, the
`classes`, `functions` and `views` hooks, the toy), and 6b, the wiring and
the root's tests (`test/api/render.mjs`, `errors.mjs`, `conversions.mjs`,
and the option objects' runtime checks step 5 left). What it found:

- napi builds an instance a function returns from the constructor it
  registered under the class's JS name, one per name. `tiny_skia.Color` and
  `svgtypes.Color` share theirs, so each is only ever returned by its own
  constructor and factories, which build from the class they are called on.
  `tiny_skia::Color::from_rgba` returns `Option<Self>` and is left out: use
  `fromRgba8`. Another class of a shared name meets the same rule.
- `Rect`, `Size` and `IntSize` keep their fields private in Rust, so they get
  no `fromObject`/`toObject` (Q31 named them): only `Transform` and
  `svgtypes.Color` are plain data. Their factories (`fromLtrb`, `fromWh`) are
  how JS makes one.
- Members returning a type the namespaced surface has no class for still
  use the flat shapes (`Rect.toNonZeroRect()` gives a `BBox`), as the usvg
  classes do (`Matrix`, `BBox`, `Dimensions`). They move with step 7 or 13.
- `usvg.Options.defaultSize` stays flattened (`defaultSizeWidth`,
  `defaultSizeHeight`): a provider field taking a `tiny_skia.Size` would
  change its shape, which is a decision of its own.

Left for later: `resvg.renderNode` and `tree.root()` (step 7), `dispose()`
on the owned classes, built on the slot that a consuming method empties
(step 8), async twins (step 9).

Step 7 added `napi_gen::project` and `Config::projections` (CONTRIBUTING,
"Projections"): `usvg.Group`, `usvg.Path`, `usvg.Image` and `usvg.Text` hold
a `napi_support::Projection` (the tree's `Arc` and the path to the node,
found again on each call), and `usvg::Node` is a union of
`{ kind, value }` objects. `usvg.ClipPath`, `usvg.Mask` and `usvg.Pattern`
are projections too. `tree.root()`, `tree.nodeById(id)`, `group.children()`,
`text.flattened()`, `group.clipPath()`, `group.mask()`, the tree's
definition tables, `clipPath.root()`, `mask.root()`, `pattern.root()` and
`resvg.renderNode` come with it (`test/api/nodes.mjs`). What it found:

- The projection machinery came out at about 420 lines in `napi-gen`
  (`project.rs`, 329 with its comments, and about 75 in `class.rs`), some
  345 without comments: over the plan's ~300, and
  was accepted. The step path itself is small: `Projection` is 60 lines of
  `napi-support`, built on closures. The rest is the `{ kind, value }`
  union both ways (an argument `&Node` reads the object back, about 90
  lines).
- Definitions are projections, not looked up: `group.clipPath()` is a step
  from the group, like `children()`, and `tree.clipPaths()` a step into the
  tree's table of `Arc`s. A first version found the `Arc` in the tree's
  table by identity instead; it is gone.
- `usvg.Image`, `usvg.Text` and `usvg.Path` were copies of the flat value
  classes and object, and `usvg.ClipPath`, `usvg.Mask` and `usvg.Pattern`
  handles. They are now projections with Rust's methods
  (`image.size()`, not `image.size`), returning `tiny_skia.Transform`,
  `tiny_skia.Rect` and `tiny_skia.Size` where they had `Matrix`, `BBox` and
  `Dimensions`. That is a breaking change of the namespaced surface, which
  is not released. `usvg.Span.underline()` and its siblings hand out a
  `usvg.Path` of their own.
- napi has no TypeScript alias for a union: `usvg.Node` is spelled out where
  it is used (`usvg.NodeGroup | usvg.NodePath | ...`).
- `Option<()>` (`render_node`) returns `true` or `null`.
- The root module is the usvg class source: `filter::Image`'s methods were
  read as `usvg::Image`'s. And another crate's type of a projected name
  (`tiny_skia_path::Path`) is not the projection.

Left for later: the SVG an `<image>` embeds
(`ImageKind::SVG(Tree)`, a tree held by value inside a node: a nested
owner), and `feImage`'s `root()` (a primitive is a plain object, copied
out of `usvg.Filter`, which stays a handle like the gradients: none of them
owns content). `layerBoundingBox()` and
`filtersBoundingBox()` still give `BBox`: `NonZeroRect` has no class.

Step 7b removed the flat node surface early (decision Q37, option D): the
projections replace it, and keeping both meant porting every later step
twice. `Resvg` keeps its parse, render, `toString`, `pendingImages` /
`pendingFonts` / `takeLogs` and the root group's boxes until step 13. What
went:

- `SvgNode` and `NodeKind`; `Resvg.node()`, `children()`, `renderNodePng`,
  `renderNodePngAsync`, `nodeBbox` and `faceOf`; `Resvg.linearGradients()`
  and the other definition tables (`usvg.Tree` has them).
- The 17 `Legacy*` classes, and the flat data types and enums nothing left on
  the flat surface names (`Path`, `Fill`, the filter primitives...). The build
  prunes them (`prune` in `build.rs`); their `usvg.*` copies stay.
- In the generator: `Config::content_view`, `Config::legacy_prefix` (no flat
  class collides any more), `Surface::Flat`, the passes' `prologue` and
  `optional`, and the `Doc` every generated class carried for the flat nodes.
  `wrap(inner)` takes the value alone. The data types a projection holds
  (`Text`'s chunks and spans) are seeded from `Config::projections` instead of
  from what the template returned.
- `fit.mjs` measures on a `usvg.Tree` (`render: (svg) => usvg.Tree`) and
  exports `nodeExtent(node)`, the old `extent()`: a group's layer box, a
  shape's or a text's stroke box, an image's box. The demo measures with it.

No new-API form yet, so dropped with the old surface: what an `feImage`
draws (`Filter.imageChildren`), the document an `<image>` embeds
(`svgChildren`), which face drew a glyph (`faceOf`, `PositionedGlyph.font`)
and a span's font families (`Font.families`). The async node render came
back with step 9 (`resvg.renderNodeAsync`); `renderNodePng`'s `background`
and sizing come back with the helpers (step 11).

Step 8 gave every class of the namespaced surface `dispose()`, `disposed`
and `[Symbol.dispose]` (CONTRIBUTING, "`dispose()` and `using`";
`test/api/dispose.mjs`). What it found:

- One mechanism covers every holding: `napi_support::Slot`, the owned
  class's `Option` grown a `disposed` flag. A `Copy` class is disposable
  too, for uniformity: nothing is freed, the slot is the flag. An `Arc` or
  a projection drops its own reference only, so a node outlives a disposed
  tree, and a disposed node, its `{ kind, value }` object included, is
  refused where a `usvg.Node` is expected.
- "Every class" is more than `class.rs` makes: the read-only views and
  handles of the passes (`usvg.Span`, `usvg.Filter`, the gradients,
  `fontdb.FaceInfo`, the `ImageKind*`...) are namespaced classes too.
  `napi_gen::dispose::slotted` puts their one field in a slot on the
  finished file, as `guard_live` does for `Resvg`, and their methods now
  return `Result<T, String>`. The `.d.ts` does not move. The toy has no
  such class, so only the root exercises that path.
- The template's `module_exports` hook `alias_dispose` (`Resvg` only)
  became a generated one over every class with a
  `dispose()`, flat `Resvg` included, whose hand-written `dispose()` stays
  until step 13. The hand-written `dtsHeader` in `package.json` became the
  generated `dts-header.d.ts` (`dtsHeaderFile`).
- The `.d.ts` header writes each interface on several lines: the api-diff
  script reads a one-line `interface X { .. }` as a statement, not as a
  merge into the class, and called the change breaking.
- `to_upstream` (option objects) throws a string code now, so a disposed
  provider (`{ fontdb: db }`) throws `Disposed`, not `GenericFailure`.

Step 9 added `Config::asynchronous` and `napi_gen::twin` (CONTRIBUTING,
"Async twins"; `test/api/async.mjs`): `usvg.Tree.fromDataAsync`,
`fromStrAsync` and `toStringAsync`, `resvg.renderAsync` and
`renderNodeAsync`, `tiny_skia.Pixmap.encodePngAsync`, and
`fontdb.Database.loadSystemFontsAsync` and `loadFontsDirAsync`. The flat
`Resvg`'s twins stay until step 13. What it found:

- A lease (Q28) is the slot's value taken out, behind a `Lease` that puts
  it back when dropped. The code is `Leased`, distinct from `Disposed` and
  `Consumed`, and `dispose()` throws it too: the value is in use, and the
  instance stays as it was. The value comes back when the task lets go of
  it on the main thread (napi's `Task::finally`), after resolving,
  rejecting or an abort while queued, never on the worker thread at the end
  of the call: a fast call finished before the next line of JS would
  otherwise make "in use" depend on timing. An `&T` of an owned class
  (`encodePngAsync`'s pixmap) is leased too: the worker reads it.
- napi's `Task` rejects with a `Status`, never a code of the binding's:
  the twin resolves with a promise rejected with the `Thrown` instead,
  which the outer promise adopts (`Settled`). The trick the flat
  `Resvg.parseAsync` used by hand, generic now.
- No `Task` type per twin: one `Work` in `napi-support` holds the call as a
  closure over its owned arguments, and its result type comes from the
  function's return (`PromiseRaw<'static, T>`), so napi writes the
  `.d.ts` (`Promise<usvg.Tree>`) without a type the generator spells out.
  `()` and `Option<()>` are written out (`Promise<void>`, `Promise<boolean |
  null>`): napi says `Promise<undefined>`.
- An argument error (a wrong class, a disposed or leased instance) throws
  at the call, as napi's own conversions do; only the call's own errors
  reject. So does a `signal` that is not an `AbortSignal` or is already
  aborted, before anything is leased.
- `dispose()` returns `Result` now, on every class (the `.d.ts` keeps
  `dispose(): void`), and its doc says it throws `Leased` on the classes an
  async call can lease.
- The toy gained `spin`, a slow function with nothing to borrow, for a call
  to queue behind deterministically; the root queues behind a slow render
  on one worker thread, as `test/async.mjs` does.

Step 10 added the `guard` module, `src/guard.rs` (CONTRIBUTING, "The
`guard` module"; `test/api/guard.mjs`): `guard.ImageHrefResolver` and
`guard.FontResolver`, the providers of `usvg.Options.imageHrefResolver` and
`fontResolver` (`Config::providers`, as `fontdb`). What it found:

- No generator hook was needed. The module is a real Rust file at the root,
  hand-written, that the template declares (`mod guard;`); the provider
  entries name its classes by ident. `build.rs` parses it beside the
  generated file only so `dispose::aliases` and `inspect_members` see its
  classes (`[Symbol.dispose]`, `util.inspect`), and stages an empty
  stand-in beside its rustfmt copy, which otherwise gives up on the
  out-of-line module and leaves `src/lib.rs` unformatted.
- The checks moved, not copied: `href_resolver`, `checked`,
  `MAX_IMAGE_PIXELS`, the fence, `font_resolver`, `Misses` and
  `image_map` left the template for `src/guard.rs`, and the flat `Resvg`
  calls them (`guard::Fence` with no directory of its own: the parse
  options' `resourcesDir`, as before).
- A provider builds a fresh usvg resolver per parse over the instance's
  `Arc`s, so the async twins needed nothing: `to_upstream` runs on the main
  thread, the resolver (`Send + Sync` boxes) moves to the worker with the
  options, and `misses()` reads what it noted once the promise settled.
  `misses()` is a snapshot, not a drain. One instance reused, concurrently
  or not, lists the misses of every parse (tested, documented): one per parse.
- The resolver's `resourcesDir` falls back to the parse options': with
  neither, no file is read.
- The default for helpers: `fit.mjs` exports `guardedParse(binding,
  options)`, a `render` for `fitTextWidths` with a fresh resolver per
  parse, which the README and `test/fit.mjs` use; the demo's
  `measureText` passes a fresh one unless its options bring one, and the
  bench's tree (behind fitting and the Nodes panel) parses through
  `guardedParse`, which now also sees the dropped images. Step 11's helpers
  take it from there.

Step 11 added `resvg-napi/helpers` (CONTRIBUTING, "The helpers";
`test/helpers/`): `helpers.browser.mjs`, which loads no binding and exports
`createHelpers(binding)`, and `helpers.mjs`, the Node entry bound to
`./index.js`; `package.json` gained `exports`. What it found:

- No Rust. `background` goes through `svgtypes.Color.fromStr` and
  `tiny_skia.Color.fromRgba8`, `maxPixels` is checked in JS before
  `new tiny_skia.Pixmap`, and the async helpers chain the generated twins
  (`fromStrAsync`, `renderAsync`, `encodePngAsync`): three worker trips for a
  parse and a render (Q29).
- Byte for byte with the flat `Resvg`, once the canvas arithmetic is Rust's:
  f32 roundings through `Math.fround`, saturating `as u32`, and a node's
  boxes as `NonZeroRect` has them (`rect.toNonZeroRect()`: its width is
  `(x + w) - x` in f32, not the `Rect`'s). `nodeExtent` uses it too.
- The flat `renderNodePng` applied a group's own transform twice: it passed
  the group's `absTransform()`, and `resvg::render_node` applies the
  group's transform again. The helper passes the parent's, and draws where
  the document cropped to the node draws (`test/helpers/node.mjs`). Groups
  without a transform of their own, and every other node, come out as before.
- System fonts: one scan per binding, shared. The async helpers scan with
  `loadSystemFontsAsync`; the sync ones scan in place on first use, and if
  both race, whichever finishes first is kept.
- The guard cannot be turned off from the helpers: a caller may pass a
  provider of its own (to read its `misses()`), and a trusted document with
  usvg's default resolvers is parsed with the raw API and rendered as a tree.
- `exports` closes deep imports: `resvg-napi/index.js` and the WASI shims are
  no longer importable by path. `browser.js` stays the `browser` condition of
  `"."`, so bundlers resolve as they did.
- `fit.mjs` is a shim over `helpers.browser.mjs` until step 13, and the
  demo's `measureText` calls the helpers' (`createHelpers(binding)`); the rest
  of the demo moves in step 12.

## Status

- [x] 1
- [x] 2
- [x] 3
- [x] 4
- [x] 4b
- [x] 5
- [x] 6
- [x] 7
- [x] 7b
- [x] 8
- [x] 9
- [x] 10
- [x] 11
- [ ] 12
- [ ] 13
- [ ] 14
