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
part is the API shape: what a `Pixmap` becomes, how images and fonts are
resolved, what runs on a worker thread.

It is a handful of files, one per phase. The generic phases are a crate of
their own, `crates/napi-gen`, a build-dependency of this one: `build.rs` fills
its `Config` and hands it over with `napi_gen::configure` before running them.

| | |
|---|---|
| `crates/napi-gen/src/sources.rs` | finding and parsing upstream: env override, then the cargo registry at the version the workspace's `Cargo.lock` names |
| `crates/napi-gen/src/vocab.rs` | what a Rust type means on the JS side — newtypes, aliases, `Deref` chains, payload enums, naming. Answers questions, emits nothing |
| `crates/napi-gen/src/emit.rs` | turning those answers into tokens napi can expand |
| `crates/napi-gen/src/class.rs` | the classes and free functions of the namespaced surface: constructors, statics, `Result` errors, owned and shared values, conversions |
| `crates/napi-gen/src/cfg.rs` | which upstream methods exist on which target: their `#[cfg(feature = ..)]`, translated through `Config::features` |
| `crates/napi-gen/src/lib.rs` | `Config`: what the phases ask the binding crate, and the `report!` macro |
| `build/config.rs` | **everything specific to resvg**: the upstream crates and their roots, the wrapped impl blocks, renames, feature gates, and the `napi_gen::Config` (`GEN`) they fill. Wrapping another crate starts by rewriting this file and `template.rs`, as `crates/toy-napi/build.rs` does |
| `build/template.rs` | **the API decisions, written by hand.** The only file here not describing upstream |
| `build.rs` | the passes, their order, the fixpoints, the assertions, and writing `src/lib.rs` |

What the template needs that has nothing to do with resvg lives in its own
crate, `crates/napi-support`: `contained` (a panic becomes a JS error), the
bounded `log` collector, capped file and gzip reads, and the checked
`AbortSignal`. It knows no upstream type, so a binding of another library can
take it as is; the template only calls into it and keeps the `#[napi]` surface.

### The generic boundary

`crates/napi-gen` and `crates/napi-support` name no wrapped crate: no `usvg`,
`resvg`, `fontdb`, `tiny_skia`, `tiny-skia` or `svgtypes`, as whole words, in
code or in comments ([ADR 0002, Q35](docs/adr/0002-api-details.md)). CI greps
for them and fails on a match. Whatever the generator has to know about
resvg is a `napi_gen::Config` field that `build/config.rs` fills: the
template's names for the domain shapes (`shapes`), the size constructor
(`size_new`), the argument stand-in and its resolver (`arg_standin`), and the
node view a definition owning a content group gets (`content_view`, whose code
is in `template.rs`), the option-object hooks (`index_bounded`,
`option_objects`, `providers`, `converters`, below), and the class and
function allowlists (`classes`, `functions`, `views`, below). A comment that
needs an example uses the toy crate's names, or none.

`crates/toy` and `crates/toy-napi` show that this holds. `toy` is a tiny
library: an `Arc`-held `Counter` with a few `&self` methods, an `Options`
struct with `Default`, a unit enum, a plain `Point`, a `Clone` `Tally`, a
`ReportOptions` struct with a field of each option-object kind, and one class
of each kind `napi_gen::class` makes (below): a plain-data `Vec2` with
`FromStr` and `Display`, an owned `Canvas` with a consuming method and a
borrowed view, two `Color`s in two modules, an error enum per fallible
function, and the free functions `report` and `paint`. `toy-napi` binds it
with the same generator, through its own `Config` and a template a few lines
long in its `build.rs`. Its `src/lib.rs` and `index.d.ts` are committed and
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

### The namespaced surface

Beside the flat exports, the generator emits a second surface with one
namespace per crate, `usvg.*` and `fontdb.*`, under the Rust names
([ADR 0002, Q21](docs/adr/0002-api-details.md)). The two coexist until step 13
of the migration removes the flat one. Three rules make it, all generic:

- **One namespace per source.** `Config::namespaces` maps a source's root to
  its namespace (`("usvg::fontdb", "fontdb")`). A type goes in the namespace
  of the source that defines it. A type only re-exported from a crate parsed
  for vocabulary (tiny-skia's) is left out until that crate has a namespace
  of its own.
- **A prefix on the Rust name.** `napi_gen::emit::namespaced` copies the data
  types, enums, unions and handle classes the passes produced. It renames
  each one with its namespace as a prefix: `FaceInfo` becomes `FontdbFaceInfo`,
  and a helper `paint_to_js` becomes `usvg_paint_to_js`. It also sets
  `js_name` and `namespace`. Two Rust items cannot share a name, so the prefix
  is needed, and JS never sees it. The one trace is the type alias napi
  declares for every class (`export type FontdbFaceInfo = FaceInfo`). The JS
  name is the upstream one, so a `data_renames` or `enum_renames` entry is
  undone. References between copies are renamed with them. Paths, fields,
  methods and enum variants are not renamed. napi resolves a reference's
  TypeScript name when the item expands, and falls back to the Rust name for
  anything not seen yet. So the definitions are emitted in dependency order,
  and impls and functions after all of them.
- **`make_mut` for `Clone` handles.** A handle class on the namespaced surface
  (`Surface::Namespaced`) whose upstream type derives `Clone` gets its
  `&mut self` methods too: they go through `Arc::make_mut`, which copies a
  value shared with another handle on the first write. That is how
  `fontdb.Database` gets `loadFontData` and the other loaders. A type that
  does not derive `Clone` stays read-only. The template's additions, the
  content view and the argument stand-in, belong to the flat surface only.

napi keeps **one constructor per JS class name**, whatever the namespace
(`REGISTERED_CLASSES` in napi 3.16). It builds every instance of a class from
the constructor registered under that name. If a namespaced class has the same
name as a flat one, one of the two hands out instances of the other, and
every method call on them throws. So `namespaced` reads the class names off
the flat surface, and when a copy would take one of them:

- with `Config::legacy_prefix` set, the **flat class gives way**: its Rust
  ident, and every reference to it in the flat code (hand-written template
  included), get the prefix, so its JS name, `.d.ts` type, inspect label and
  `toStringTag` follow. The root sets `Some("Legacy")`: `ClipPath` becomes
  `LegacyClipPath` and `usvg.ClipPath` keeps the upstream name (decision Q36).
  That is the 17 usvg classes (`ClipPath`, `Text`, `Span`...), and they go
  with the flat surface in step 13. The codegen report lists each one as
  `legacy flat name`. A flat class with a `js_name` of its own fails the
  build: the rename would have to edit it.
- with `None`, the copy is left out, with anything that refers to it, and the
  report lists it as `not namespaced`.

Two namespaces offering one class name always leave the second out. Objects
and string enums have no constructor and are not affected.

The toy shows both rules. `toy.Tally` derives `Clone` and is tested for
copy-on-write. `toy.Counter` is namespaced beside a flat `LegacyCounter`, and
the test checks that instances of the flat one still work.
`test/api/namespaces.mjs` checks the root's namespaces against `index.d.ts`:
every class and enum it declares exists at runtime under its Rust name.

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
out: it is to become a class with a constructor (step 6 of the migration). A
field maps as the flat options' fields do, or through one of three hooks:

- **Providers** (`Config::providers`): "a field of type `T` is supplied by an
  instance of class `C` providing it". The field takes the instance as a
  `ClassInstance<'a, C>`, so the object gets a lifetime, and the provider's
  expression makes the value out of it. `Options::fontdb` takes a
  `fontdb.Database` and shares its `Arc`. napi checks the class: another
  class's instance, or a plain object, throws (natively by type tag, on WASI by
  the class recorded at wrap time). The resolvers (ADR 0002, Q26) get their
  provider classes in step 10; until then a field with no JS form and no
  provider is left out of the type and keeps its default, and the report lists
  it as `no provider`.
- **Converters** (`Config::converters`): "a field of type `T` takes the JS
  type `J`, converted by the template function `f`". For a type the generator
  cannot read, such as xmlwriter's `Indent`, a payload enum of a crate it does not
  parse: `indent: number | 'tabs' | 'none'`, read by `indent_from_js`, which
  the flat `WriteOptions` uses too. A bad value throws.
- **Index-bounded fields** (`Config::index_bounded`): a `u8` field indexing an
  upstream `static` table without a bounds check, by its name suffix and the
  table (`*_precision` and `POW_VEC`). It is clamped to the table's last index,
  read off the table by `emit::index_bounds`, on the flat and the namespaced
  options alike, and the build fails when the suffix matches nothing.

An argument `&O` of a listed option struct takes the object, optional: left
out, it is `O::default()`. The toy tests the conversion through
`toy.report(tally, options)`, its generated free function: omitted fields
equal `Default`, the provider field takes only a `toy.Tally`, the converter,
and the clamp.

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
  read-only `Arc`. Step 8 of the migration builds `dispose()` on the owned
  slot.
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

If you are adding to the API, `template.rs` is where it goes. If upstream moved
and something stopped mapping, it is `vocab.rs` or `emit.rs`, and the codegen
report says which member and why.

The split is verifiable rather than a matter of taste: `src/lib.rs`,
`index.d.ts` and `codegen-report.txt` are committed and CI diffs them, so a
refactor of the generator is correct exactly when the output does not move. That
is how this one was done — five files out of one, six generated files
byte-identical afterwards.

`src/lib.rs`, `index.js` and `index.d.ts` are generated *and* committed, and the
generator is deterministic: CI regenerates them and fails on any diff. A
version bump is therefore readable as a diff: moving between resvg 0.47 and
0.48.1 — usvg, fontdb and tiny-skia along with it — changes five lines of
`src/lib.rs`, all of them doc comments, and the test suite passes untouched.
What that diff cannot show is behaviour: the same bump moves 390 pixels of
`photo-card.png`, all inside the colour emoji. The examples are there to catch
that half.

The generator keeps no signature guards of its own: what the template calls by
hand (`resvg::render`, `Tree::from_data`, `Tree::to_string`, the resolver
fields) is checked by rustc when `src/lib.rs` compiles, and an upstream move
shows up there or as a diff in the committed output. Additions need no edit: a
new `Options` field or enum variant appears in the bindings, and in TypeScript,
on its own.

Async is derived too. A method that belongs on a worker thread is marked once,
where it is defined:

```rust
#[async_twin(render_png_async, Buffer)]
fn png_bytes(&self, params: Option<RenderParams>) -> Result<Vec<u8>> { … }
```

and the generator writes the rest from the signature: the `Task` type, the
`AbortSignal` wiring, the public `#[napi]` method and its
`Promise<Buffer>` return type, plus a doc comment that names the sync sibling it
found by looking for the method that calls the core. Five twins come out of five
marks — `renderPngAsync`, `renderRawAsync`, `renderNodePngAsync`,
`toStringAsync`, and `SvgNode.renderPngAsync` — where two of them used to be
~25 lines of ceremony each and the other three did not exist.

The mark carries the two things a rule cannot know: *whether* the thread hop is
worth it, and what the result looks like in JS. Everything else is read off the
core. The core exists because `Buffer` holds a JS handle and is not `Send`: it
returns `Vec<u8>`, `String` or `(u32, u32, Vec<u8>)`, and a three-impl `IntoJs`
trait converts on the main thread in `Task::resolve`. Adding a fourth output
shape means adding one impl, not touching the rule.

Two things about `AbortSignal` that the tests had to learn the hard way: napi
listens for the abort *event*, so a signal that is already aborted when the task
is scheduled is ignored; and `Task::compute` is not interruptible, so abort
drops a *queued* task and never stops a running one. `test/async.mjs` therefore
occupies the single-thread pool with a blocker before it aborts anything.

`Resvg.parseAsync` and the free `renderAsync` stay hand-written: they have no
receiver to capture — they build the object — so they do not share the shape.

Two hand-written lists remain, and both are decisions a rule cannot make: four
method names skipped as noise (`isolate`, `should_isolate`, `id`, `subroots`)
and eight renames (`from_data` → the constructor, `node_by_id` → `node`,
`to_string` → `to_svg_string`, because Rust already has `Display::to_string`).
Everything else about what the template covers is read *off the template*: it is
built once with no generated methods, parsed with `syn`, and its own method
names become the "covered" set. A stale rename fails the build; a method
emitted twice fails it in rustc.

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
npm run typecheck     # tsc --strict over index.d.ts and test/types.mts
npm run demo          # builds the wasm bundle and serves demo/ on :8787
```

`test:wasi` points `index.js` at `resvg-napi.wasi.cjs` through
`NAPI_RS_NATIVE_LIBRARY_PATH`, so the same thirteen files exercise the wasm
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
`index.js`, `index.d.ts`, `browser.js` and the five WASI shims.

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

One version lives in seven places, and every one of them has failed a release
at least once:

```bash
npm pkg set version=X.Y.Z
npm run version        # napi version -> the thirteen npm/* manifests
npm pkg set optionalDependencies.resvg-napi-linux-x64-gnu=X.Y.Z   # ...and the twelve others
npm pkg set peerDependencies.resvg-napi-wasm32-wasi=X.Y.Z
sed -i 's/^version = ".*"/version = "X.Y.Z"/' Cargo.toml           # the crate, and Cargo.lock with it
npm run build          # index.js embeds the version in its binding checks
npm install --package-lock-only
```

`napi version` touches the thirteen manifests and nothing else. The dependency
maps that name those packages are left alone: `napi pre-publish` rewrites
`optionalDependencies` inside the publish job, but `check:package` reads the
committed manifest and fails a release whose root still points at the previous
version -- and `peerDependencies` is rewritten by nobody at all, which is how
0.3.0 shipped asking for the 0.2.0 wasm package and answering ERESOLVE to
anyone who installed both.

`index.js` carries the version in fifty-four generated strings (`expected X.Y.Z
but got ...`), so a bump without a rebuild trips the drift check. `Cargo.toml`
is the one that looks cosmetic and is not: `CARGO_PKG_VERSION` is hashed into
every class's napi type tag, so two releases sharing a crate version stamp
identical tags and accept each other's objects without an error.

Commit all of it, then push a `v*` tag. CI builds
every target, publishes the platform packages before the root one, and creates
the GitHub release with the pull requests merged since the last tag as its body.

Then one more commit, once the packages are on npm:

```bash
npm install --package-lock-only   # the platform packages, at the new version
```

The bump ran that command before the new version existed, so npm left the
thirteen `node_modules/resvg-napi-*` entries out of the lockfile, and `npm ci`
fails on `main` until they are back (0.6.0 shipped that way; #122 restored
them).

There is no `CHANGELOG.md` any more. It was a file to remember at every pull
request, and it recorded what a release body is for. The generated list of pull
requests is the *what*, with the `breaking-api` ones under **Breaking**; the
*why*, and what a user has to change, is written by hand at the top of the
release once CI has opened it.
