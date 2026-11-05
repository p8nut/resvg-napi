# ADR 0002: details of the mirrored API

- Status: accepted, 2026-11-05. Refines [ADR 0001](0001-thin-generic-wrapper.md).
- Implemented by the plan in [`docs/migration-0.10.md`](../migration-0.10.md).

These are the questions ADR 0001 left open once the generator was read. Each
numbered item matches the decision log (Q21–Q35).

## Names (Q21)

- Types keep their Rust names and live in the namespace of the crate that
  **defines** them. For example `fontdb.FaceInfo` (not `FontFace`),
  `tiny_skia.Transform` (not `Matrix`), `tiny_skia.Rect` and
  `tiny_skia.Size` (not `BBox` and `Dimensions`).
- A re-exported type appears once, under its defining crate. `usvg.Color`
  and `tiny_skia.Color` are different types, and the namespaces keep them
  apart.
- Methods are camelCase (`node_by_id` becomes `nodeById`).
- napi keys classes by Rust identifier, so the binding prefixes identifiers
  with the crate (`UsvgColor`) and sets `js_name` and `namespace`. The prefix
  is invisible from JS.

## Tree nodes (Q22)

`usvg.Node` is a discriminated union, `{ kind: 'group' | 'path' | 'image' |
'text', value }`, which is the direct translation of the Rust enum. Each
value is a projection handle: it keeps the tree alive and re-resolves the
node on each call.

## Errors (Q23)

- A `Result<T, E>` whose `E` is an enum throws (sync) or rejects (async).
  `code` is the variant name and `message` is the Rust `Display`, unchanged.
- Each error enum gets a TypeScript type of its codes. The hand-written
  `ParseErrorCode` goes away.
- Friendlier wording, such as the SVGZ advice from #167, belongs to the
  helpers.

## Option-returning functions (Q24)

- `new` is the JS constructor. It throws when Rust returns `None`, because a
  JS `new` cannot return `null`.
- Every other function returns `T | null` for `Option<T>`.

## Large allocations (Q25)

The raw API has no size limit. `new tiny_skia.Pixmap(w, h)` with a huge size
aborts the process, exactly as in Rust, and the docs say so. The helpers
refuse anything over `maxPixels` (2^26) before allocating.

## Resolvers (Q26)

- `usvg.Options.imageHrefResolver` and `fontResolver` take an instance of a
  **provider class**. A provider is hand-written Rust in the resvg-specific
  `guard` module, for example
  `new guard.ImageHrefResolver({ images, resourcesDir })`, with `misses()` to
  read back unresolved hrefs.
- Without a provider, usvg's default applies.
- The generator mechanism is generic: "this field accepts an instance of a
  class that provides the Rust type".
- Use one provider instance per parse. The helpers create a fresh one each
  time.

## Fonts (Q27)

- `usvg.Options.fontdb` takes a `fontdb.Database` instance. If that does not
  work inside a plain options object, it falls back to a trailing argument;
  PR 5 starts with a spike to find out.
- Without one, the raw API uses an **empty** database, as Rust does, so text
  does not render. The docs say so, with an example.
- The helpers load the system fonts once, cached and asynchronously.

## Mutable arguments in async calls (Q28)

A value taken as `&mut` by an async twin, such as `Pixmap` in
`resvg.renderAsync`, is **leased**:
- it moves into the task;
- it comes back in `finally`, whether the task succeeds, fails or is aborted;
- any access in between throws `in use by an async call`.

`Arc`-backed values, such as `Tree`, are cloned and need no lease.

As built (step 9): the error's `code` is `Leased`, and `dispose()` throws
it too, leaving the instance as it was. An owned value the call only reads
(`Pixmap` in `encodePngAsync`) is leased as well, since it cannot be shared
with the worker thread.

## Render-then-encode (Q29)

There is no generated chain. Rendering and encoding a PNG off the main
thread takes two worker round trips, which the helpers chain. A chain is added
only if a measurement justifies it (ADR 0001's Rust-helper rule).

## fontdb members left out of 0.10 (Q30)

`query`, `faceData`, `familyName`, `faceSource` and `families` leave in 0.10.
They need two generator rules: owned mirrors of borrowed argument types, and
callback arguments whose result is returned. **They must come back**:
tracked in #181, and named in the 0.10 release notes.

## Plain-data classes and conversions (Q31, Q34)

- `tiny_skia.Transform` is a class with its Rust associated functions and
  methods (`fromScale`, `preTranslate`, …).
- Every class whose fields are all public and convertible gets a static
  `fromObject(obj)` and an instance `toObject()`, for example `Transform`,
  `Rect`, `Size` and `svgtypes.Color`.
- Arguments accept only the class, so a conversion is always explicit.
- Conversions the generator **invents** come in pairs: there is never
  `fromObject` without `toObject`.
- Conversions that come from a Rust trait follow Rust, one direction at a
  time: `FromStr` gives a static `fromStr()` that throws on failure, and
  `Display` gives `toString()`. One does not require the other, and nothing
  reports the asymmetry: Rust ships one-way types on purpose.

## Helpers outside Node (Q32)

`resvg-napi/helpers` works in Node with no setup. In a browser, the loaded
binding is passed in: `createHelpers(binding)`. A `browser` condition in
`package.json` `exports` picks the variant.

## CSS colours (Q33)

`svgtypes` is exposed, starting with `svgtypes.Color`, whose `fromStr` is
generated from `FromStr`. svgtypes is already a direct dependency. The
helpers parse `background` with it.

## Boundary grep (Q35)

The CI grep that keeps the generic part free of `usvg`, `resvg`, `fontdb`,
`tiny_skia`, `tiny-skia` and `svgtypes` covers comments too: whole words, case
sensitive. Examples in comments use the toy crate.
