# ADR 0001: a thin wrapper that mirrors the Rust crates, on a generic generator

- Status: accepted, 2026-11-05. Implemented by the 0.10.0 migration.
- Replaces: the hand-written `Resvg` class API of 0.1–0.9.

## Context

Through 0.9.0 the JS API was a hybrid. Part of it is generated and mirrors
usvg and fontdb almost one to one (`SvgNode`, `Path`, `Text`,
`FontDatabase`...). The rest is about 1,800 lines of `build/template.rs`
with no Rust counterpart: the `Resvg` class, `renderPng(params)`,
`resolveImage`, `pendingImages`, per-document logs, `faceOf`. That half grows
with every convenience and ties the generator to resvg.

The package has no known users, so breaking the API costs nothing today. The
goal is a simple, direct wrapper of the Rust crates, generic enough that the
generator could one day bind another crate.

## Decision

### The API mirrors the crates

- One namespace per crate: `usvg`, `resvg`, `fontdb`, `tiny_skia`. Rust names
  in camelCase.
- The shapes follow Rust. For example, parsing is
  `usvg.Tree.fromData(data, options)`, and rendering is
  `resvg.render(tree, transform, pixmap)` on a `tiny_skia.Pixmap`, then
  `pixmap.encodePng()`. The `Resvg` class goes away.
- Option structs are plain objects. Missing fields take the `Default` impl's
  values. A struct without `Default` stays a class with a constructor.
- Hand-written code covers only what crossing into JS forces: type
  conversion, worker threads, errors, panics. It is written generically.

### Helpers sit beside it, in JS

- `resvg-napi/helpers` holds the conveniences, for example
  `renderPng(svg, { scale, background, fonts })`. It is written in JS on top
  of the generated API. `fit.mjs` moves there, and the `resvg-napi/fit` entry
  point goes away.
- A helper moves to Rust only when one of these forces it:
  - a measured number of JS↔Rust crossings, for example walking a large tree
    getter by getter;
  - a `Send + Sync` callback, such as usvg's image and font resolvers.
  
  That Rust lives in a separate module that is clearly resvg-specific. It is
  never part of the generic code.
- Making a call atomic off the main thread (render then encode in one worker
  trip) is the generator's job, for every crate. It is not done by hand.
- The demo uses only the helpers, which makes it their real-world test.

### Safety

- Panics are contained everywhere (generic).
- The resvg-specific guards become an opt-in resolver and options, and the
  helpers turn them on by default. The guards are:
  - file reads fenced to `resourcesDir`, with `O_NOFOLLOW` and a 256 MiB cap;
  - at most 2^28 pixels per raster image;
  - SVGZ refused.
- The raw API behaves like Rust. Like usvg's defaults, it reads any path and
  decodes any image.

### Generic and resvg-specific code are split

- The generic part is the generator and `napi-support`. It must never name
  `usvg`, `resvg`, `fontdb` or `tiny_skia`.
- The resvg part is the generator config (which types and functions to expose,
  which get `*Async` twins), the resvg-specific Rust module and the JS helpers.
- Both parts stay in this repository. A separate package waits until a second
  crate shows what really needs to be configurable.
- These stay generic, for every class:
  - readable `util.inspect` output and `toStringTag`;
  - `*Async` twins (worker and `AbortSignal`) for the functions the config
    lists;
  - `dispose()` / `using`, with a `disposed` check in every method;
  - log collection, global only (per-document logs go away with `Resvg`).

### CI

- A step classifies the `index.d.ts` diff of each pull request as either
  additions only, or removals and changed signatures. It comments on the pull
  request and, in the second case, adds the `breaking-api` label. With an API
  that mirrors upstream, every bump of resvg, usvg, fontdb or tiny-skia can
  change the public surface.
- Two checks guard the generic boundary:
  - a grep fails when the generic part names a resvg crate;
  - a toy crate is run through the generator, its binding is built, and a
    small JS test runs against it.
- The update checks stay the same: the daily `lock` job, `resvg.yml`
  bump pull requests, and Dependabot npm groups. Their pull requests get the
  label above, and they test the helpers and the toy crate.

### Migration and version

- Successive pull requests on `main`. The old and new APIs coexist during the
  migration, and there is no intermediate release.
- The result ships as 0.10.0. 1.0 waits until the API has settled in use.

## Consequences

- To learn the API, read the Rust docs: the JS surface follows them.
- Upstream API changes reach users directly. The `breaking-api` label and
  the release trigger (a release after every update round that touches the
  binary or the API) keep that visible.
- The resvg-specific hand-written Rust is confined to a single module that
  must be justified. `build/template.rs` shrinks to generic plumbing.
