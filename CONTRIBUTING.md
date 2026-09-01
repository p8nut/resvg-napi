# Contributing

Almost none of `src/lib.rs` is written by hand. Read this before changing it --
or rather, before changing the generator in `build.rs` and `build/`, which is
what writes it.

## How the bindings are generated

The generator parses the sources of `usvg`, `resvg`, `fontdb`, `tiny-skia-path`
and `strict-num` with `syn`, and emits `src/lib.rs` with `quote`. Everything that
describes upstream — types, names, signatures, doc comments, even bounds such as
the precision clamp read out of usvg's `POW_VEC` — is derived. The hand-written
part is the API shape: what a `Pixmap` becomes, how images and fonts are
resolved, what runs on a worker thread.

It is five files, one per phase:

| | |
|---|---|
| `build/sources.rs` | finding and parsing upstream: env override, then git submodule, then the cargo registry at the version `Cargo.lock` names |
| `build/vocab.rs` | what a Rust type means on the JS side — newtypes, aliases, `Deref` chains, payload enums, naming. Answers questions, emits nothing |
| `build/emit.rs` | turning those answers into tokens napi can expand |
| `build/template.rs` | **the API decisions, written by hand.** The only file here not describing upstream |
| `build.rs` | the passes, their order, the fixpoints, the assertions, and writing `src/lib.rs` |

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

The generator does not take upstream's word for anything it templates against.
Four guards run before emission — `resvg::render`'s full signature,
`Tree::from_data`, `Tree::to_string`, and the fields of `ImageHrefResolver` and
`FontResolver` — so a rename fails the build with
`usvg::ImageHrefResolver::resolve_string disappeared; update build.rs` instead
of generating something plausible. Additions need no guard: a new `Options`
field or enum variant appears in the bindings, and in TypeScript, on its own.

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
names become the "covered" set. A stale rename fails the build; so does a method
emitted twice.

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
cargo fmt --check
cargo clippy --release --all-targets -- -D warnings
npm run check:pins:selftest      # the Cargo.toml pins, offline half
npm run check:package:selftest   # what publish would ship, offline half
npm run check:resvg:selftest     # the version probe the weekly bump uses
npm run report:selftest          # the codegen report snapshot
npm run conformance:selftest     # the corpus runner, offline half
npm run ci:targets:selftest      # the build matrix and its tiers
```

CI runs the lint on `x86_64-unknown-linux-gnu` only -- the toolchain is warm
there and nothing here is target-specific enough to lint thirteen times -- and
regenerates the committed bindings to fail on any diff: `src/lib.rs`,
`index.js`, `index.d.ts`, `browser.js` and the five WASI shims.

It does not build all thirteen targets on a pull request. It builds
`x86_64-unknown-linux-gnu`, which everything above runs on, and
`wasm32-wasip1-threads`, which is a different code path. The other eleven build
on the merge, on a `v*` tag, on a manual dispatch, or on a pull request labelled
`full-matrix`. Adding a target means an entry in `scripts/ci-targets.mjs` and
the triple in `package.json` under `napi.targets` -- the script fails when the
two disagree -- then `npm run create-npm-dirs`.

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
derived surface on purpose:

```bash
npm run report -- -w
```
