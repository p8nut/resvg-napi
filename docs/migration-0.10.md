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
| 5 | Option structs as plain objects with `Default`, plus field providers for fields with no JS form. Start with a spike on the `fontdb` field. | `fontdb` field |
| 6 | Factories, free functions and `Result` errors. Owned and consuming classes. `resvg`, `tiny-skia` (allowlist: `Pixmap`, `Color`, `IntSize`, `Transform`, `Rect`, `Size`) and `svgtypes` (`Color`) become sources. `FromStr` gives `fromStr`, `Display` gives `toString`, and plain-data classes get `fromObject`/`toObject`. | allowlist creep |
| 7 | Generic projections over borrowed values: `tree.root()`, `group.children()` returning `usvg.Node` unions, `resvg.renderNode`. | highest: stop and decide if the generic step path passes about 300 lines |
| 8 | Generic `dispose()` / `disposed` / `using` for every class, with a generated `.d.ts` header. | low |
| 9 | Async twins listed in config, with leases for `&mut` arguments. | the put-back path on abort |
| 10 | The resvg-specific `guard` module (hand-written, outside the grep): `ImageHrefResolver`, `FontResolver`, `resourcesDir` fence, 256 MiB, 2^28 px, SVGZ refused. | one provider per parse |
| 11 | `resvg-napi/helpers` in JS: `parse`, `renderPng`/`renderPngAsync`, `renderRaw`, `renderNodePng`, `measureText`, `fit`, system fonts, `maxPixels`, `background` through `svgtypes.Color.fromStr`, the friendlier SVGZ message, `createHelpers(binding)`, and `package.json` `exports`. | `exports` closes deep imports |
| 12 | Demo and conformance move to the helpers and the new API. | conformance baseline moves |
| 13 | Remove the old API. Must come out labelled `breaking-api`. | large: split into 13a (Rust) and 13b (JS, tests, docs) if needed |
| 14 | Release 0.10.0. The notes name what left (#181). | — |

## Status

- [x] 1
- [x] 2
- [x] 3
- [ ] 4
- [ ] 5
- [ ] 6
- [ ] 7
- [ ] 8
- [ ] 9
- [ ] 10
- [ ] 11
- [ ] 12
- [ ] 13
- [ ] 14
