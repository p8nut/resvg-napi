# Contributing

Almost none of `src/lib.rs` is written by hand. Read this before changing it --
or rather, before changing the generator in `build.rs`, `build/` and
`crates/napi-gen`, which is what writes it.

Where the project is heading is recorded as decisions in [`docs/adr/`](docs/adr/).
[ADR 0001](docs/adr/0001-thin-generic-wrapper.md) makes the API a direct
mirror of the Rust crates, with JS helpers beside it and a generator kept
generic, and [ADR 0002](docs/adr/0002-api-details.md) settles the details.
Check new work against them. The steps to 0.10.0 are in
[`docs/migration-0.10.md`](docs/migration-0.10.md).

## How the bindings are generated

The generator parses the sources of `usvg`, `resvg`, `fontdb`, `tiny-skia-path`
and `strict-num` with `syn`, and emits `src/lib.rs` with `quote`. Everything that
describes upstream — types, names, signatures, doc comments, even bounds such as
the precision clamp read out of usvg's `POW_VEC` — is derived. The hand-written
part is a list of decisions in `build/config.rs` (which types are classes,
which calls get a worker-thread twin, which option fields take a class) and the
resvg-specific guards of `src/guard.rs`.

It is a handful of files, one per phase. The generic phases are a crate of
their own, `crates/napi-gen`, a build-dependency of this one: `build.rs` fills
its `Config` and hands it over with `napi_gen::configure` before running them.

| | |
|---|---|
| `crates/napi-gen/src/sources.rs` | finding and parsing upstream: env override, then the cargo registry at the version the workspace's `Cargo.lock` names |
| `crates/napi-gen/src/vocab.rs` | what a Rust type means on the JS side — newtypes, aliases, `Deref` chains, payload enums, naming. Answers questions, emits nothing |
| `crates/napi-gen/src/emit.rs` | turning those answers into tokens napi can expand: data types, unions, option objects, the namespaces, the prelude every generated file starts with |
| `crates/napi-gen/src/class.rs` | the classes and free functions of the namespaced surface: constructors, statics, `Result` errors, owned and shared values, conversions |
| `crates/napi-gen/src/project.rs` | the projections: classes over values borrowed from an owner, and `{ kind, value }` unions of them |
| `crates/napi-gen/src/dispose.rs` | `dispose()` for the classes `class.rs` does not make, `[Symbol.dispose]` set at load, and the `.d.ts` header declaring it |
| `crates/napi-gen/src/twin.rs` | the `*Async` twins `Config::asynchronous` lists: arguments made owned on the main thread, the call on a worker thread, leases |
| `crates/napi-gen/src/cfg.rs` | which upstream methods exist on which target: their `#[cfg(feature = ..)]`, translated through `Config::features` |
| `crates/napi-gen/src/lib.rs` | `Config`: what the phases ask the binding crate, and the `report!` macro |
| `build/config.rs` | **everything specific to resvg**: the upstream crates and their roots, the allowlists, the hooks, feature gates, and the `napi_gen::Config` (`GEN`) they fill. Wrapping another crate starts by rewriting this file and `template.rs`, as `crates/toy-napi/build.rs` does |
| `build/template.rs` | **the resvg-specific items of the generated file, by hand**: the crate paths, `mod guard;`, and `indent_from_js`, the one converter `GEN` names. About thirty lines |
| `src/guard.rs` | **the resvg-specific guards, written by hand**: the `guard` providers (below) |
| `build.rs` | the passes, their order, the fixpoints, the assertions, and writing `src/lib.rs` |

What the generated code needs at run time that has nothing to do with resvg
lives in its own crate, `crates/napi-support`: `contained` (a panic becomes a
JS error), the bounded `log` collector (global: `setLogLevel` and `takeLogs`,
which `napi_gen::emit::prelude` emits for every binding), capped file reads,
the checked `AbortSignal`, `Projection` (a value borrowed from an `Arc`-held
owner, below), `Slot` (a class's value until a consuming call or `dispose()`,
and the `Lease` an async call takes of it) and `spawn` (an async call on a
worker thread, as a promise). It knows no upstream type, so a binding of
another library can take it as is.

### The generic boundary

`crates/napi-gen` and `crates/napi-support` name no wrapped crate: no `usvg`,
`resvg`, `fontdb`, `tiny_skia`, `tiny-skia` or `svgtypes`, as whole words, in
code or in comments ([ADR 0002, Q35](docs/adr/0002-api-details.md)). CI greps
for them and fails on a match. Whatever the generator has to know about
resvg is a `napi_gen::Config` field that `build/config.rs` fills: the
option-object hooks (`index_bounded`, `option_objects`, `providers`,
`converters`, below), the class and function allowlists (`classes`,
`functions`, `views`, below), the types borrowed from an owner
(`projections`, below), the types seen as what they iterate (`iterated`,
below), and the members given an async twin (`asynchronous`, below). A
comment that needs an example uses the toy crate's names, or none.

`crates/toy` and `crates/toy-napi` show that this holds. `toy` is a tiny
library: an `Arc`-held `Counter` with a few `&self` methods, an `Options`
struct with `Default`, a unit enum, a plain `Point`, a `Clone` `Tally`, a
`ReportOptions` struct with a field of each option-object kind, and one class
of each kind `napi_gen::class` makes (below): a plain-data `Vec2` with
`FromStr` and `Display`, an owned `Canvas` with a consuming method and a
borrowed view, two `Color`s in two modules, an error enum per fallible
function, the free functions `report` and `paint`, and a `Scene` whose
`Layer`s and `Dot`s are only ever borrowed from it, with an enum `Item` of
the two and the free functions `count` and `visit` that take one,
`spin`, slow on purpose, for the async twins to queue behind, and the tuple
rule's three shapes: `Tally::pairs` (`Vec<(String, u32)>`), `Vec2::parts`
(`(f32, f32)`) and the `Trail` a scene hands out, seen as the `Step`s it
iterates (`Config::iterated`), a union whose `Line(Vec2, Vec2)` carries a
tuple of plain-data classes.
`toy-napi` binds it
with the same generator, through its own `Config` and a template a few lines
long in its `build.rs`. Its `src/lib.rs`, `index.d.ts` and `dts-header.d.ts` are committed and
drift-checked like the root's. The linux-gnu CI job builds it and runs its
tests:

```bash
npx napi build --cwd crates/toy-napi --platform --no-js
node --test crates/toy-napi/toy.spec.mjs
```

The test is a `.spec.mjs` beside the crate rather than in a `test/` directory,
for the same reason the scripts' tests are: the root's `node --test` would
pick it up. Neither toy crate is published, and neither is in the npm tarball
or the third-party notices. If a change to the generator breaks the toy, the
change is resvg-shaped. Fix the generator, not the toy.

### The namespaces

The generator emits one namespace per crate, `usvg.*`, `fontdb.*`,
`tiny_skia.*`, `svgtypes.*` and `resvg.*`, under the Rust names
([ADR 0002, Q21](docs/adr/0002-api-details.md)). The root holds nothing else
but `setLogLevel` and `takeLogs` (and the hand-written `guard.*`). Three rules
make it, all generic:

- **One namespace per source.** `Config::namespaces` maps a source's root to
  its namespace (`("usvg::fontdb", "fontdb")`). A type goes in the namespace
  of the source that defines it. tiny-skia, svgtypes and resvg are sources of
  the classes and functions `GEN` lists only (`config::CLASS_CRATES`), never
  of discovery, and tiny-skia is read with tiny-skia-path, whose
  `Transform`, `Rect` and `Size` it re-exports: they are `tiny_skia.*`. A type
  usvg only re-exports from them is not `usvg.*`.
- **A prefix on the Rust name.** `napi_gen::emit::namespaced` places the data
  types, enums, unions and classes the passes produced. It renames each one
  with its namespace as a prefix: `FaceInfo` becomes `FontdbFaceInfo`, and a
  helper `paint_to_js` becomes `usvg_paint_to_js`. It also sets `js_name` and
  `namespace`. Two Rust items cannot share a name, so the prefix is needed,
  and JS never sees it. The one trace is the type alias napi declares for
  every class (`export type FontdbFaceInfo = FaceInfo`). References between
  items are renamed with them. Paths, fields, methods and enum variants are
  not renamed. napi resolves a reference's TypeScript name when the item
  expands, and falls back to the Rust name for anything not seen yet. So the
  definitions are emitted in dependency order, and impls and functions after
  all of them.
- **`make_mut` for `Clone` handles.** A handle class (`emit::wrapper_class`)
  whose upstream type derives `Clone` gets its `&mut self` methods too: they
  go through `Arc::make_mut`, which copies a value shared with another handle
  on the first write. A type that does not derive `Clone` stays read-only.

napi keeps **one constructor per JS class name**, whatever the namespace
(`REGISTERED_CLASSES` in napi 3.16). It builds every instance of a class from
the constructor registered under that name. If two classes share a name, one
of the two hands out instances of the other, and every method call on them
throws. So a second namespace offering a class name is left out, with
anything that refers to it, and the report lists it as `not namespaced`,
unless both are classes only ever built by the class they are called on
(`Classes::shared_names`, below). Objects and string enums have no
constructor and are not affected. This is why `tiny_skia::Path` is no class:
`usvg.Path` has the name, and `usvg.Path.data()` would return it.

The toy shows the rules. `toy.Tally` derives `Clone` and is tested for
copy-on-write. `test/api/namespaces.mjs` checks the root's namespaces against
`index.d.ts`: every class and enum it declares exists at runtime under its
Rust name, and nothing else is at the root.

Until 0.10 a flat surface (the hand-written `Resvg` class, `FontDatabase`,
`RenderOptions`, the `BBox` / `Matrix` / `Dimensions` / `PathSegment`
shapes) sat beside the namespaces; step 13 of the migration removed it, and
the generator hooks that served only it (`shapes`, `size_new`,
`arg_standin`, `hand_wired_fields`, `hand_wrapped`, the renames, the
`#[async_twin]` marker, the template's method passes).

### Option objects

An upstream option struct is a plain object on the namespaced surface
([ADR 0001](docs/adr/0001-thin-generic-wrapper.md)): `usvg.Options`,
`usvg.WriteOptions`. `Config::option_objects` lists the structs, and
`napi_gen::emit::option_object` makes each one an input-only
`#[napi(object, object_to_js = false)]` with every field optional, plus a
`to_upstream(&self) -> Result<Upstream>` that starts from upstream's
`Default` and overrides only the fields JS provided. A field left out, or
`undefined`, keeps the default, whatever upstream says it is: for `usvg.Options`
that is an empty font database, Times New Roman and the default resolvers, as in
Rust (ADR 0002, Q27). A listed struct without `Default` is reported and left
out: it is to become a class with a constructor. A field maps by its type
(numbers, strings, string lists, enums, paths), or through one of three
hooks:

- **Providers** (`Config::providers`): "a field of type `T` is supplied by an
  instance of class `C` providing it". The field takes the instance as a
  `ClassInstance<'a, C>`, so the object gets a lifetime, and the provider's
  expression makes the value out of it. `Options::fontdb` takes a
  `fontdb.Database` and shares its `Arc`; `imageHrefResolver` and
  `fontResolver` take the `guard` classes (below, ADR 0002, Q26);
  `defaultSize` a `tiny_skia.Size` (Q39: its fields are private, so no
  `fromObject`; JS writes `tiny_skia.Size.fromWh(w, h)`). napi checks
  the class: another class's instance, or a plain object, throws (natively by
  type tag, on WASI by the class recorded at wrap time). A field with no JS
  form and no provider is left out of the type and keeps its default, and the
  report lists it as `no provider`.
- **Converters** (`Config::converters`): "a field of type `T` takes the JS
  type `J`, converted by the template function `f`". For a type the generator
  cannot read, such as xmlwriter's `Indent`, a payload enum of a crate it does not
  parse: `indent: number | 'tabs' | 'none'`, read by the template's
  `indent_from_js`. A bad value throws.
- **Index-bounded fields** (`Config::index_bounded`): a `u8` field indexing an
  upstream `static` table without a bounds check, by its name suffix and the
  table (`*_precision` and `POW_VEC`). It is clamped to the table's last index,
  read off the table by `emit::index_bounds`, and the build fails when the
  suffix matches nothing.

An argument `&O` of a listed option struct takes the object, optional: left
out, it is `O::default()`. The toy tests the conversion through
`toy.report(tally, options)`, its generated free function: omitted fields
equal `Default`, the provider field takes only a `toy.Tally`, the converter,
and the clamp.

### The `guard` module

`src/guard.rs` is hand-written and resvg-specific, like the template, so it
lives at the root, outside the boundary grep ([ADR 0001](docs/adr/0001-thin-generic-wrapper.md),
"Safety"). The generated `src/lib.rs` declares it (`mod guard;`, from the
template), and `build.rs` reads it beside the generated file so its classes
get `[Symbol.dispose]` and `util.inspect` like the others. It holds:

- `guard.ImageHrefResolver({ images, resourcesDir, resourcesRoot })`, the
  provider of `usvg.Options.imageHrefResolver`: supplied buffers first, then
  files under `resourcesDir` (its own, else the parse options') through
  `napi_support::fs::read_regular` (`O_NOFOLLOW`, regular files, 256 MiB),
  canonicalised against `resourcesRoot`; gzip data (SVGZ) and raster images
  declaring more than 2^28 px refused and logged; `misses()`.
- `guard.FontResolver()`, the provider of `fontResolver`: usvg's
  selectors, noting the named families the database lacks; `misses()`.
- the functions both are built on (`href_resolver`, `font_resolver`,
  `Misses`, `image_map`).

A provider makes a fresh usvg resolver per parse, sharing the instance's
images and misses (`Arc`s): an async twin moves it to the worker, and what
the worker notes is read on the main thread after the promise settles. One
instance per parse is the rule; one reused, even concurrently, lists the
misses of every parse it served.

**The raw default is unsafe by design.** Without a provider, `usvg.Options`
keeps usvg's `Default` resolvers, as Rust does (Q10): they read any path an
`<image href>` names and decode any image. `test/api/guard.mjs` proves the
raw parse reads a file outside `resourcesDir`, and the guard does not. So
every helper that parses caller SVG passes a fresh provider: the helpers'
`parse` and everything built on it, and `guardedParse(binding, options)`.
The demo parses only through the helpers.

### The helpers

`resvg-napi/helpers` is JS on the generated API, nothing else
([ADR 0001](docs/adr/0001-thin-generic-wrapper.md), "Helpers sit beside it,
in JS"). Two files and their hand-written declarations:

- `helpers.browser.mjs` (`helpers.browser.d.mts`) loads no binding.
  `createHelpers(binding)` builds the set on the binding it is given, one set
  per binding (a `WeakMap`), so the system fonts are scanned once per binding.
  It also holds the binding-free functions: `nodeExtent`, `fitTextWidths(svg,
  render)`, `findWidthConstraints`, `guardedParse` (the 0.9 `fit.mjs`, gone
  in 0.10).
- `helpers.mjs` (`helpers.d.mts`) is the Node entry: the same exports, plus
  the set bound to `./index.js`, as the default export and as named exports.

`package.json` `exports` picks one: `"./helpers"` resolves to
`helpers.browser.mjs` under the `browser` condition (a bundler), to
`helpers.mjs` otherwise. `"."` keeps `browser.js` for bundlers and `index.js`
for Node, and `"./package.json"` is readable.
Everything else is closed: a new public file needs an entry, and
`npm run check:package` fails when an `exports` target is not in the tarball.

The rules the helpers keep:

- **The guard is the default.** Every parse gets a fresh
  `guard.ImageHrefResolver` and `guard.FontResolver`, unless the caller
  passes one. There is no option that turns it off.
- **Same sizes as 0.9's `Resvg`.** `renderPng`, `renderRaw` and
  `renderNodePng` redo the 0.9 Rust `canvas()` in JS: the same f32 roundings
  (`Math.fround`), the same saturating casts, the same order of checks and the
  same messages. Until 0.10 `test/helpers/` compared their bytes with the flat
  render over every sizing path; it now holds the sizes that render computed,
  frozen (`SIZES`, and a digest of a 2400-size sweep), and compares the
  pixels with the raw API's. A change to that arithmetic moves them: say why
  in the pull request.
- **`maxPixels` before allocating.** The size is checked before
  `new tiny_skia.Pixmap`, which aborts the process on a failed allocation
  (ADR 0002, Q25).
- **`nodeExtent` is a plain box**, `{ x, y, width, height }`: the numbers of
  the `tiny_skia.NonZeroRect` usvg hands out, which `crop` takes back as it
  takes a `Rect` or a `NonZeroRect`.
- **No new Rust.** A helper moves to Rust only under ADR 0001's rule (measured
  crossings, or a `Send + Sync` callback), into the resvg-specific code, never
  the generator.

Two consumers keep both surfaces honest, one each:

- **The demo uses the helpers** (`demo/README.md`): `createHelpers(binding)`
  on the wasm module in the pages, `resvg-napi/helpers` by its package name
  in `demo/render.mjs`, and the namespaced API only where the helpers stop
  (the `fontdb.Database`, the Nodes panel's tree walk, `takeLogs()`). It is
  their real-world test: something the demo needs and the helpers lack is a
  finding, not a reason to reach for the raw API around them. `npm run
  test:examples` renders the four examples with `--strict`.
- **The conformance harness uses the raw API**, call for call as upstream's
  `crates/resvg/tests/integration/main.rs`: one `fontdb.Database` of the
  corpus fonts (sorted, the five generic families set),
  `usvg.Tree.fromData(data, { fontdb, resourcesDir })` with usvg's default
  resolvers, `size().toIntSize().scaleToWidth(300)`, `resvg.render` with
  `Transform.fromScale` on each axis, then `takeDemultiplied()`. No guard:
  upstream has none, and the corpus reaches only `tests/resources` through
  its `../../../resources` hrefs. `npm run conformance:fetch` gets the corpus
  at the tag `Cargo.lock` builds, and `npm run conformance` compares with
  `conformance/baseline.txt` (`-- -w` rewrites it). A moved baseline is
  investigated before it is rewritten.

### Classes and free functions

`napi_gen::class` mirrors the types `Config::classes` lists as classes of the
namespaced surface, and the functions `Config::functions` lists as functions
of their source's namespace. Both are allowlists by source root and name:
nothing becomes a class or a function on its own. A class gets every
associated function and method whose signature maps, with Rust's names in
camelCase ([ADR 0002](docs/adr/0002-api-details.md), Q21, Q23, Q24, Q31,
Q34):

- **`new` is the constructor**, and throws when Rust returns `None` (a JS
  `new` cannot return `null`, Q24) or `Err`. Every other associated function
  is a static. `Option<T>` is `T | null`.
- **A `Result<T, E>` throws** an `Error` whose `code` is the variant's name
  when `E` is an enum of a source, and whose `message` is `E`'s `Display`,
  unchanged (Q23). Each such enum gets a string enum of its codes,
  `<E>Code`, in its own namespace. An error type no source defines (`std::io::Error`)
  throws with the code `GenericFailure`. Every generated function returns
  `Result<T, String>`, napi's `Result` with a string code
  (`napi_support::Thrown`); a number that does not fit the upstream integer
  throws `InvalidArg` instead of wrapping.
- **How a class holds its value is read off the type** (`class::Holding`): a
  `Copy` type by value; a type with a method that consumes `self`, or with
  `&mut self` methods and no `Clone`, owned until that method takes it, after
  which every call throws `Consumed`; a `Clone` type with `&mut self` methods
  behind an `Arc`, copied on write (`fontdb.Database`); anything else behind a
  read-only `Arc`. Whatever it is, it sits in a `napi_support::Slot`, which
  `dispose()` empties (below).
- **Arguments**: numbers, booleans, strings and bytes; a listed class, by
  value (`Copy`), `&` or `&mut` as its holding allows; an option object; and a
  **view** (`Config::views`): `&mut V` that a class's method returns for the
  call, so `paint(view: &mut CanvasView)` takes the `Canvas` and calls
  `as_view()` on it. A generic `P: AsRef<Path>` or `S: Into<String>` is a
  string.
- **Conversions** come from Rust's traits, one direction each and with
  nothing reported when the other is missing (Q34): `FromStr` gives a static
  `fromStr(s)` that throws, `Display` gives `toString()`. A class whose fields
  are all public and of a number, boolean or string type is **plain data**:
  it gets a getter and a setter per field, and `fromObject(obj)` and
  `toObject()` over a `<Name>Object` interface, always both (Q31).
- **A class held by value is that class wherever a member names it**
  (`Js::Class`): a getter or a method of a data type, a handle or a
  projection returning `Transform`, `NonZeroRect` or `Size` hands out the
  `tiny_skia` class, built by its `__from`. Not when a wrapped crate defines
  a type of that name itself (`usvg::Color` is not `tiny_skia.Color`). Where
  a class cannot go (an `#[napi(object)]` field, a union's payload), a
  plain-data class is its `<Name>Object`, and any other makes its holder a
  read-only class (`usvg.Primitive`, which holds a `NonZeroRect`).
- **Tuples are arrays** (Q38): `Vec<(A, B)>` or `&[(A, B)]` is
  `Array<[A, B]>`, each element mapped as a union's payload is (a face's
  `families`, `Array<[string, fontdb.Language]>`); a tuple of numbers,
  booleans or strings returned is `[A, B]` (`IntSize.dimensions()`); a
  payload variant with several unnamed fields carries them as `value: [a, b]`.
- **Iterated types** (`Config::iterated`): a type with no JS form of its own,
  whose content one of its methods iterates, is the array of what that
  method yields: `tiny_skia_path::Path` is its `segments()`, the
  `PathSegment` union, so `usvg.Path.data()` is
  `Array<{ type: 'moveTo', value: PointObject } | ...>`.
- Panics are contained (`napi_support::contained`), and `unsafe fn`s are left
  out.

Two namespaces may each have a class of one name (`toy.Color` and
`alt.Color`, `tiny_skia.Color` and `svgtypes.Color`). napi builds an instance
it returns from the constructor registered under the class's JS name, and it
keeps one per name, so such a class (`Class::shared_name`) is only ever
returned by its own constructor and its factories (`#[napi(factory)]`, which
build from the class they are called on). Anything else that would return
one, `toy::Color::checked`'s `Option<Self>` for instance, is left out and
reported. `namespaced` lets those names through (`Classes::shared_names`).
The Rust items stay apart because `namespaced` keys them by namespace: a name
means its own namespace's item first, then the one other namespace's that
defines it.

### `dispose()` and `using`

Every class of the namespaced surface has `dispose()` and a `disposed`
getter ([ADR 0001](docs/adr/0001-thin-generic-wrapper.md), "These stay
generic"). After `dispose()` every other member throws an `Error` whose
`code` is `Disposed`, and so does passing the instance as an argument or as
an option object's provider field. A second `dispose()` does nothing.

- **One mechanism for every holding**: the value sits in a
  `napi_support::Slot`, the `Option` a consuming method already emptied
  (`Consumed`), and `dispose()` empties it too (`Disposed`, which wins over
  `Consumed`). By holding: an owned value (`tiny_skia.Pixmap`) is freed
  there and then; an `Arc` (`usvg.Tree`, `fontdb.Database`) or a projection
  (`usvg.Group`, the `value` of a `usvg.Node`) drops this instance's
  reference, and the owner lives on while another instance holds it, a
  node reached from a disposed tree included; a `Copy` value
  (`tiny_skia.Transform`) has nothing to free, and is disposable for
  uniformity: the slot is its flag.
- `class.rs` makes the slot and `dispose()` of the classes it emits.
  `napi_gen::dispose::slotted` does the same afterwards for the others,
  the read-only views and handles of the passes (`usvg.Span`,
  `usvg.LinearGradient`, `fontdb.FaceInfo`...): such a class has one field,
  which it puts in a slot, and each `#[napi]` method takes the value from
  the slot first and returns `Result<T, String>` (napi unwraps it: the
  `.d.ts` does not move).
- **`[Symbol.dispose]`** is set when the module loads, to the prototype's
  `dispose`, on every class with one, the `guard` classes included
  (`napi_gen::dispose::aliases`, a `#[napi(module_exports)]` hook). Only
  where the runtime has the symbol: napi's `well_known_symbol` would fail
  the whole module without it (Node before 20.4, iOS Safari as of 2026), so
  it is never declared to napi. `test/api/dispose.mjs` loads the module
  with a `Symbol` that lacks it.
- **The type**: napi declares no `[Symbol.dispose]`, so the build writes
  `dts-header.d.ts`, which `package.json` names as napi's `dtsHeaderFile`:
  napi copies it to the top of `index.d.ts`. It merges an interface into
  every class, under its namespace:

  ```ts
  export declare namespace usvg {
    export interface Tree {
      [Symbol.dispose](): void
    }
  }
  ```

  so `new tiny_skia.Pixmap(1, 1) satisfies Disposable` (`test/types.mts`).
  It is generated and committed like `index.d.ts`, and CI fails on a diff.
- `util.inspect` of a disposed instance prints `Name { disposed: true }`.

### Projections

Some upstream values are only ever borrowed: a tree's nodes live inside the
tree, behind `Box`es, and nothing hands out an `Arc` of one. A class cannot
hold a borrow, so `Config::projections` lists these types and
`napi_gen::project` mirrors them as projections
([ADR 0002, Q22](docs/adr/0002-api-details.md)):

- **A struct is a class holding a `napi_support::Projection`** (`Holding::Projected`):
  the owner's `Arc`, type-erased, and the path from it to the value. Each call
  finds the value again from the owner, and the instance keeps the owner
  alive after JS dropped it. The owner is never written (a copy-on-write
  `Arc` copies first), so a path that resolved once always does. Its `&self`
  methods map as any class's do. A copy handed out elsewhere (`From<&T>`, a
  value class's `wrap`) is a projection of a value of its own.
- **An enum whose variants each carry one of those structs** (`V(Box<S>)` or
  `V(S)`) is a union of `{ kind, value }` objects, one per variant
  (`<Enum><Variant>`): `kind` is the variant's name in camelCase, `value` the
  projected class. The value remembers the enum it was reached through, so an
  argument `&Enum` takes the object back (`<enum>_from_js`). A value not
  reached as a variant, or of another class than `kind` says, throws.
- **A method returning `&T`, `Option<&T>`, `&[T]` or `&[Arc<T>]`** of a
  projected struct or union, on a projected class or on a class behind an
  `Arc` (the owner), is a step: the projection calls the method again on
  each use, and goes through the `Arc` of a table's element. Its arguments
  move into the step, so they must be values (a string, a number). A type
  reached both ways (from a table and from what refers to it) is one class.

`Option<()>` returns `true` or `null` (`boolean | null` in the `.d.ts`): the
value says whether the call did anything.

The root lists `usvg::Node` and the four node types: `tree.root()` is a
`usvg.Group`, `group.children()` and `tree.nodeById(id)` hand out
`usvg.NodeGroup | usvg.NodePath | usvg.NodeImage | usvg.NodeText`, and
`resvg.renderNode(node, transform, pixmap)` takes one back. It lists the
definitions that own content too, `ClipPath`, `Mask` and `Pattern`: from the
tree's tables (`tree.clipPaths()`) or from what refers to them
(`group.clipPath()`, `clipPath.clipPath()`, `mask.mask()`), and their
`root()` is a `usvg.Group` of the same tree. The toy tests the rules on its
`Scene` (a tree of `Layer`s and `Dot`s, and a table of `Arc<Dot>`),
including that a node outlives its owner's JS object.

### Async twins

`Config::asynchronous` lists the members that get an async twin, by source
root and name: `("usvg", "Tree::from_data")` for a static or a method,
`("resvg", "render")` for a free function. `napi_gen::twin` writes
`<name>Async(...args, signal?)` beside the sync member, returning a promise
([ADR 0001](docs/adr/0001-thin-generic-wrapper.md); [ADR 0002, Q23, Q28,
Q29](docs/adr/0002-api-details.md)). The root lists parsing
(`usvg.Tree.fromDataAsync`, `fromStrAsync`), writing back (`toStringAsync`),
rendering (`resvg.renderAsync`, `renderNodeAsync`), encoding
(`tiny_skia.Pixmap.encodePngAsync`) and loading fonts
(`fontdb.Database.loadSystemFontsAsync`, `loadFontsDirAsync`). There is no
render-then-encode chain (Q29): two round trips, which the helpers chain.

- **Arguments are read on the main thread**, into values a worker thread can
  own: bytes and strings are copied, an option object becomes its upstream
  struct, a class behind an `Arc` or a projection is cloned (`usvg.Tree`, a
  `usvg.Node`), a `Copy` class copied (`tiny_skia.Transform`). An argument
  error (a wrong class, a disposed instance) throws at the call, as napi's
  own conversions do.
- **The call runs on the worker thread**, panics contained, through
  `napi_support::spawn` (napi's `AsyncTask`, so libuv's pool natively and
  the WASI host's under WASI). The result is made JS on the main thread. A
  `Result` error rejects with the `code` and `message` the sync member
  throws: the promise resolves with a promise rejected with that error,
  which it adopts, because napi's own rejection keeps only a `Status` for
  the code.
- **A leased value** (Q28): a class held by value and borrowed (`&T` of an
  owned class, `&mut T` of any, a `&mut` view such as the pixmap
  `resvg.render` draws on) cannot be copied or shared, so it goes with the
  call. `napi_support::Slot::lease` takes it out of the slot; until the
  promise settles, every use of the instance throws an `Error` whose `code`
  is `Leased` (`this Pixmap is in use by an async call`), and so does
  `dispose()`, which leaves the instance as it was. The value comes back
  however the call ends: resolved, rejected, or dropped unrun because its
  signal fired while it was queued. The `Lease` puts it back when dropped,
  and the slot takes it once the task let go of its hold, which napi does
  on the main thread after settling (`Task::finally`): the instance is
  usable again from the first line after the `await`, never earlier, so a
  fast call cannot slip past a check. A copy-on-write class (`fontdb.Database`
  loading fonts) is leased the same way: its `Arc` goes with the call, and
  other handles on the same value keep reading theirs.
- **`signal`** is checked before anything is read (`napi_support::Signal`):
  not an `AbortSignal`, or already aborted, it throws. Aborting drops a call
  still queued, which rejects with an `AbortError` and never runs; a call
  already running finishes, as `Task::compute` cannot be interrupted.
  `test/api/async.mjs`, like `test/async.mjs`, runs with
  `UV_THREADPOOL_SIZE=1` (the WASI loader reads it too) and a slow render in
  front, so the call it aborts is queued.
- A listed member that cannot have a twin fails the build, with the reason:
  a constructor (list a factory), a return that borrows from the receiver,
  an argument with no owned form, a class with a shared name returned
  (napi builds it from the sync factory's constructor only). Its
  `.d.ts` is napi's, from the return type (`Promise<usvg.Tree>`), with
  `Promise<void>` and `Promise<boolean | null>` written out for `()` and
  `Option<()>`.

The toy has one of each: a static that throws (`Canvas.fromTextAsync`),
free functions taking a leased view (`paintAsync`), a union (`countAsync`)
and numbers (`spinAsync`, slow on purpose, to queue behind), and methods
leasing an owned receiver (`fillAsync`), leasing a copy-on-write one
(`pushAsync`) and cloning an `Arc` (`joinedAsync`).

If you are adding to the API, `build/config.rs` is where it goes: a class,
a function or a twin is a line of an allowlist, and the generator does the
rest. If upstream moved and something stopped mapping, it is `vocab.rs` or
`emit.rs`, and the codegen report says which member and why.

The split is verifiable rather than a matter of taste: `src/lib.rs`,
`index.d.ts` and `codegen-report.txt` are committed and CI diffs them, so a
refactor of the generator is correct exactly when the output does not move.

`src/lib.rs`, `index.js` and `index.d.ts` are generated *and* committed, and the
generator is deterministic: CI regenerates them and fails on any diff. A
version bump is therefore readable as a diff: moving between resvg 0.47 and
0.48.1 — usvg, fontdb and tiny-skia along with it — changes five lines of
`src/lib.rs`, all of them doc comments, and the test suite passes untouched.
What that diff cannot show is behaviour: the same bump moves 390 pixels of
`photo-card.png`, all inside the colour emoji. The examples are there to catch
that half.

The generator keeps no signature guards of its own: what the template and the
guard call by hand (`Indent`, the resolver fields) is checked by rustc when
`src/lib.rs` compiles, and an upstream move shows up there or as a diff in the
committed output. Additions need no edit: a new `Options` field, method of a
listed class or enum variant appears in the bindings, and in TypeScript, on
its own.

Two things about `AbortSignal` that the tests had to learn the hard way: napi
listens for the abort *event*, so a signal that is already aborted when the task
is scheduled is ignored (`napi_support::Signal` refuses it up front); and
`Task::compute` is not interruptible, so abort drops a *queued* task and never
stops a running one. `test/api/async.mjs` therefore occupies the single-thread
pool with a blocker before it aborts anything.

To build against a checkout instead of the crates.io cache:

```bash
USVG_SRC_DIR=/path/to/resvg/crates/usvg/src cargo build --release
```

## Scripts

```
npm run build         # napi build --platform --release
npm test              # the suite on the native binding, then the four examples
npm run test:wasi     # the same suite against the wasm build (POSIX shells)
npm run test:examples # renders demo/examples/*.svg headlessly
npm run conformance   # resvg's corpus against its references (conformance:fetch first)
npm run typecheck     # tsc --strict over the .d.ts / .d.mts files and test/*.mts
npm run demo          # builds the wasm bundle and serves demo/ on :8787
```

`test:wasi` points `index.js` at `resvg-napi.wasi.cjs` through
`NAPI_RS_NATIVE_LIBRARY_PATH`, so the same suite exercises the wasm
build. That run is why nothing in the suite assumes an installed font: WASI has
no font directories, and `loadSystemFonts()` finds nothing there. Tests take
the database and the family name from `test/support.mjs`, which falls back to a
font file (`RESVG_TEST_FONT`, or a few well-known paths) and skips loudly when
there is none. The env-var prefix is POSIX-shell syntax; on Windows use
`npm test`, which CI does too.


## Lint and guards

```
cargo fmt --all --check
cargo clippy --workspace --release --all-targets -- -D warnings
node --test "scripts/*.spec.mjs" # the scripts' own tests, offline
npm run check:package            # what publish would ship: root tarball, manifests
```

The scripts' tests are `*.spec.mjs`, not `*.test.mjs`: plain `node --test`,
which `npm test` runs, would pick the latter up on every test-matrix entry.

The compiler is the one `rust-toolchain.toml` pins, locally and in CI: rustup
installs it on first use. A new stable lands through that file, in its own pull
request -- the daily `lock` job goes red when one is out -- so a clippy release
cannot turn unrelated pull requests red.

CI runs the lint on `x86_64-unknown-linux-gnu` only -- the toolchain is warm
there and nothing here is target-specific enough to lint thirteen times -- and
regenerates the committed bindings to fail on any diff: `src/lib.rs`,
`index.js`, `index.d.ts` and its `dts-header.d.ts`, `browser.js` and the five
WASI shims.

It does not build all thirteen targets on a pull request. It builds
`x86_64-unknown-linux-gnu`, which everything above runs on, and
`wasm32-wasip1-threads`, which is a different code path. The other eleven build
on the merge, on a `v*` tag, on a manual dispatch, or on a pull request labelled
`full-matrix`. Adding a target means the triple in `package.json` under
`napi.targets`, then `npm run create-npm-dirs`; `scripts/ci-targets.mjs` reads
that list and derives the host and cross flag from the triple.

The codegen keeps a report of everything it derived and everything it left
alone, with the reason. It is off by default, because it rides on
`cargo::warning` and buried real warnings:

```bash
RESVG_NAPI_CODEGEN_LOG=1 cargo build --release
```

That report is the first place to look when a member is missing from the
TypeScript surface: it says which member, and why.

It is snapshotted in `codegen-report.txt`, and CI fails on a diff -- the same
treatment the generated bindings get, because the README points at that report
as the answer to "why is this member missing". After a change that moves the
derived surface on purpose, regenerate it and commit the diff:

```bash
npm run report
```

## The `breaking-api` label

The API mirrors upstream, so a resvg, usvg, fontdb or tiny-skia bump can move
it without anyone writing a line here. Every pull request's `index.d.ts` diff is
classified, and the label says the result:

- **none**: no declaration added, removed or changed. Doc comments and
  whitespace do not count.
- **additive**: additions only -- a new declaration, an optional parameter
  appended, a union alias that gained a member, an interface merged into a
  class of the same name.
- **breaking**: anything else -- a declaration removed (a move between
  namespaces is one), a signature changed in any way (narrowed, retyped,
  optional made required), a union or enum member dropped, a required field
  added to an existing interface.

`scripts/api-diff.mjs` keys every declaration as `namespace.container.member`
with its normalised signature and compares the two sets; anything changed that
is not one of the widenings above is breaking, on purpose. To see what a branch
does:

```bash
git show origin/main:index.d.ts > /tmp/base.d.ts
node scripts/api-diff.mjs /tmp/base.d.ts index.d.ts
```

`.github/workflows/api.yml` runs it on each pull request against the merge
base, sets or clears `breaking-api`, and keeps one comment with the summary up
to date (it only starts one when there is something to say). It runs as
`pull_request_target`, so it can label pull requests from forks, and for that
reason it runs nothing from the pull request: its scripts come from `main`, and
the head's `index.d.ts` is read as data with `git show`. A change to it or to
the script therefore takes effect after the merge, not on its own pull request.
`resvg.yml` applies the label to the bumps it opens itself, because a pull
request opened with `GITHUB_TOKEN` starts no workflow.

The label is what the release notes are sorted by: `.github/release.yml` puts
labelled pull requests under **Breaking** in the generated body. Every update
round that touches the binary or the API ends in a release, so that section is
where a user learns what upstream moved. The label can be set or removed by
hand when the classifier is wrong; the next push to the pull request
reclassifies it.

## Third-party notices

`THIRD-PARTY-NOTICES.md` is generated from `Cargo.lock` and committed, the same
treatment `codegen-report.txt` gets and for the same reason: the crate graph
moves under you, most often on a resvg bump. Every `.node` and the `.wasm`
statically link 91 crates, and MIT, BSD-2, BSD-3, ISC and Unicode-3.0 all ask
for their notice to travel with a binary. After a dependency changes:

```bash
npm run licenses
```

That writes the snapshot *and* copies it, with `LICENSE-APACHE` and
`LICENSE-MIT`, into all thirteen `npm/*` packages. The copies are the point: a
notice about what a `.node` contains has to ship in the package that holds the
`.node`, and the root package holds none -- it ships JavaScript and type
declarations. Shipping it only from the root, as 0.3.0 did, put it in the one
package none of that code is in. `files[]` names all three in every platform
manifest -- `npm run licenses` writes that entry as well as the file,
because `napi create-npm-dirs` rewrites those manifests from `napi.targets` and
would otherwise drop the notices without saying so.

CI checks it beside the codegen report, in the linux build job: reading the
graph needs a toolchain and a warm registry, which the package job has neither
of. A bump that moves the graph fails there rather than shipping a notice that
no longer describes the binary -- or a platform package left holding the
previous one.

## Releasing

A release is a tag, nothing else:

```bash
git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z
```

The repository stays at version 0.0.0. On a `v*` tag, CI runs
`scripts/set-version.mjs` with the tag's version before it builds, packs and
publishes. That one script writes the version into the seven places it
lives, and every one of them has failed a release at least once:
- `package.json`, its `optionalDependencies` and the wasm `peerDependencies`
  (0.3.0 shipped asking for the 0.2.0 wasm package);
- the thirteen `npm/*` manifests;
- `Cargo.toml` and `Cargo.lock`. `CARGO_PKG_VERSION` is hashed into every
  class's napi type tag, so two releases sharing a crate version accept each
  other's objects without an error;
- `index.js`, whose binding checks name the version.

The build job then checks that `napi build` reproduces exactly what the script
wrote. Nothing is committed: no release pull request, and no lockfile pull
request after it. The platform packages are never in `package-lock.json`,
which CI's `npm install` tolerates.

CI builds every target, publishes the platform packages before the root one,
and creates the GitHub release with the pull requests merged since the last
tag as its body.

There is no `CHANGELOG.md` any more. It was a file to remember at every pull
request, and it recorded what a release body is for. The generated list of pull
requests is the *what*, with the `breaking-api` ones under **Breaking**; the
*why*, and what a user has to change, is written by hand at the top of the
release once CI has opened it.
