//! Writes src/lib.rs from the toy crate's sources with `napi-gen`: the generic
//! generator, on a crate it was not written for. Everything specific to `toy`
//! is in `GEN` and the small template below.

use std::{env, fs, path::PathBuf, process::Command};

use napi_gen::class::{Classes, Holding, Source};
use napi_gen::emit::*;
use napi_gen::sources::{ModTree, parse_crate};
use napi_gen::vocab::{Vocab, payload_enums};
use napi_gen::{Config, Converter, Iterated, Provider, View};
use quote::quote;

/// The toy has no font tag and no feature gates: those answers are empty.
const GEN: Config = Config {
    log_env: "TOY_NAPI_CODEGEN_LOG",
    // `ReportOptions::style_index` indexes `STYLES` unchecked.
    index_bounded: &[("_index", "STYLES")],
    // `ReportOptions` and `Options` become `toy.ReportOptions` and
    // `toy.Options`. `Counter` has no `Default`, so it is refused (it is a
    // class with a constructor instead).
    // `Filter` borrows a list of `Tag`s, an input enum: `toy.Filter` holds
    // `toy.Tag` objects, converted for the call.
    option_objects: &["ReportOptions", "Options", "Counter", "Filter"],
    // `ReportOptions::base` is supplied by a `toy.Tally`.
    providers: &[Provider {
        ty: "Arc<Tally>",
        class: "Tally",
        provide: "v.__ref()?.clone()",
        doc: "A `toy.Tally`. Left out: an empty one.",
    }],
    // `ReportOptions::width` is a payload enum: `width_from_js` below reads
    // a number or `'auto'`.
    converters: &[Converter {
        ty: "Width",
        js: "Either<u32, String>",
        ts: "number | 'auto'",
        convert: "width_from_js",
    }],
    tag4_types: &[],
    // A `Trail` is what its `steps()` iterates: `scene.trail()` is an array
    // of `Step` unions.
    iterated: &[Iterated {
        ty: "Trail",
        via: "steps",
        item: "Step",
    }],
    node_types: &[],
    not_data: &[],
    features: &[],
    // The crate root's items under `toy`, the `alt` module's under `alt`.
    namespaces: &[("toy", "toy"), ("toy::alt", "alt")],
    // One class of each holding: `Counter` shared, `Tally` copied on
    // write, `Vec2` and `Color` by value, `Canvas` owned. `Vec2` and
    // `alt::Color` are plain data, and the two `Color`s share a JS name.
    classes: &[
        ("toy", "Counter"),
        ("toy", "Tally"),
        ("toy", "Vec2"),
        ("toy", "Color"),
        ("toy", "Canvas"),
        ("toy::alt", "Color"),
        ("toy", "Scene"),
        ("toy", "Key"),
    ],
    functions: &[
        ("toy", "report"),
        ("toy", "paint"),
        ("toy", "count"),
        ("toy", "visit"),
        ("toy", "spin"),
    ],
    // Async twins, one of each shape: a static that throws
    // (`Canvas.fromTextAsync`), free functions taking a view of an owned
    // class (`paintAsync`, which leases the canvas), a union
    // (`countAsync`) and numbers (`spinAsync`, slow enough to queue
    // behind), and methods on each holding that crosses differently: an
    // owned receiver leased for `&mut` (`fillAsync`), a copy-on-write one
    // leased (`pushAsync`) and an `Arc` cloned (`joinedAsync`).
    asynchronous: &[
        ("toy", "Canvas::from_text"),
        ("toy", "Canvas::fill"),
        ("toy", "Tally::push"),
        ("toy", "Tally::joined"),
        ("toy", "paint"),
        ("toy", "count"),
        ("toy", "spin"),
    ],
    // `paint` writes through a `CanvasView`, which JS hands over as the
    // `Canvas` it borrows.
    views: &[View {
        ty: "CanvasView",
        class: "Canvas",
        via: "as_view",
    }],
    // A `Scene`'s nodes are borrowed from it: `Layer` and `Dot` become
    // classes holding a projection, and `Item` a union of the two.
    projections: &[("toy", "Item"), ("toy", "Layer"), ("toy", "Dot")],
};

fn main() {
    napi_build::setup();
    napi_gen::configure(GEN);
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/lib.rs");

    let here = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = here.join("../toy/src");
    let all = parse_crate(&src);
    let tree = ModTree::load(&src, &all);
    // The crate root's files and the `alt` module's: a source each.
    let (alt, files): (Vec<_>, Vec<_>) = tree
        .public_view(&all, &src)
        .into_iter()
        .partition(|(p, _)| tree.file_ns.get(p).is_some_and(|ns| ns == "alt"));
    let lib: Vec<syn::File> = files
        .iter()
        .filter(|(p, _)| p.ends_with("lib.rs"))
        .map(|(_, f)| f.clone())
        .collect();
    let files: Vec<syn::File> = files.into_iter().map(|(_, f)| f).collect();
    let alt: Vec<syn::File> = alt.into_iter().map(|(_, f)| f).collect();
    let (modules, root) = (&tree.paths, quote!(toy));

    // The classes and functions (`GEN.classes`, `GEN.functions`), each
    // source's into its namespace. Built first: a member naming a class held
    // by value is that class (`Vocab::classes`).
    let none = Default::default();
    let classes = Classes::new(vec![
        Source {
            root: "toy",
            files: &files,
            modules,
        },
        Source {
            root: "toy::alt",
            files: &alt,
            modules: &none,
        },
    ]);

    // The unit enum, the plain-data struct and the payload enum `Step`,
    // each through the emitter the root crate uses for the same shape.
    let mut vocab = Vocab::default();
    // `Color` has two, so a member's bare `Color` names neither.
    for (name, plain, _) in classes.by_value().filter(|(n, _, _)| *n != "Color") {
        vocab.classes.insert(name.to_string());
        if plain {
            vocab.plain.insert(name.to_string());
        }
    }
    let wanted = ["Direction".to_string()].into();
    let (enums, enum_code, _) = map_enums(&files, &wanted, modules, &root);
    vocab.enums = enums;
    vocab.objects.insert("Point".into());
    let (point, skipped, _) = object_struct(&files, "Point", &vocab, modules, &root);
    assert!(
        !point.is_empty() && skipped.is_empty(),
        "Point maps completely"
    );
    // A unit variant, one carrying a plain-data class (its `Vec2Object`),
    // and one carrying two (a tuple, `value: [from, to]`).
    let step = payload_enums(&files, "", &Default::default(), "toy")
        .remove("Step")
        .expect("Step is a payload enum");
    vocab.payload.insert("Step".into(), step);
    assert!(
        vocab.payload["Step"].payload_blocker(&vocab).is_none(),
        "every Step variant maps"
    );
    let step =
        payload_enum_code("Step", &vocab.payload["Step"], &vocab, modules).expect("the Step union");
    let bounds = index_bounds(&files);
    assert_eq!(bounds, [("_index", 2)], "STYLES has three entries");

    // `Tag`, the input enum: `{ type, value }`, borrowed for the call.
    let mut tag = quote!();
    for e in files.iter().flat_map(|f| &f.items).filter_map(|i| match i {
        syn::Item::Enum(e) if is_enum_input(e) => Some(e),
        _ => None,
    }) {
        assert_eq!(e.ident, "Tag", "Tag is the one input enum");
        tag.extend(enum_input(e, modules, &root));
    }
    vocab.ints.insert("MinLen".into());

    // The option objects: `ReportOptions` with every field optional over
    // `Default`, `base` taken from a `toy.Tally` (a provider), `width`
    // through `width_from_js` (a converter), `style_index` clamped to
    // `STYLES`. `finish` is a function pointer, with no JS form and no
    // provider: left out, it keeps its default. `Options` maps whole.
    // `Counter` has no `Default`.
    let mut options = quote!();
    for name in GEN.option_objects {
        match option_object(&files, name, &vocab, modules, &root, &bounds) {
            Some(obj) => {
                let skipped: Vec<_> = obj.skipped.iter().map(ToString::to_string).collect();
                let (want, bounded): (&[&str], usize) = match *name {
                    "ReportOptions" => (
                        &["finish: fn(String)->String (no provider: keeps the default)"],
                        1,
                    ),
                    _ => (&[], 0),
                };
                assert_eq!(
                    has_input_lists(&files, name),
                    *name == "Filter",
                    "{name}: only Filter borrows lists"
                );
                assert_eq!(skipped, want, "{name}: what is left out");
                assert_eq!(obj.bounded, bounded, "{name}: style_index is clamped");
                options.extend(obj.code);
            }
            None => assert_eq!(*name, "Counter", "{name} has Default"),
        }
    }

    // What a class leaves out is listed, as the codegen report would.
    let holdings: Vec<_> = classes.classes.iter().map(|c| c.holding).collect();
    assert_eq!(
        holdings,
        [
            Holding::Shared,
            Holding::SharedCow,
            Holding::Value,
            Holding::Value,
            Holding::Owned,
            Holding::Value,
            Holding::Shared,
            Holding::Value,
            Holding::Projected,
            Holding::Projected,
        ],
        "each class holds its value as its shape asks"
    );
    let plain: Vec<_> = classes
        .classes
        .iter()
        .filter(|c| !c.plain.is_empty())
        .map(|c| &c.name)
        .collect();
    assert_eq!(plain, ["Vec2", "Color"], "plain data: Vec2 and alt::Color");
    assert_eq!(
        classes.shared_names(),
        ["Color".to_string()].into(),
        "two namespaces have a Color"
    );
    let mut toy_part = quote! { #enum_code #point #step #tag #options };
    let mut alt_part = quote!();
    for (i, c) in classes.classes.iter().enumerate() {
        let (code, skipped) = classes.class(i, &vocab);
        let skipped: Vec<_> = skipped.iter().map(ToString::to_string).collect();
        let want: &[&str] = match (classes.sources[c.source].root, c.name.as_str()) {
            // `Option<Self>` outside the constructor: `null` would have to
            // go through napi's constructor registry, which holds one
            // `Color` of the two.
            ("toy", "Color") => &[
                "checked (returns Self, a class whose JS name another namespace has: \
                 only its constructor and factories may)",
            ],
            // A view has no JS form of its own: `paint` borrows it.
            ("toy", "Canvas") => &["as_view (returns CanvasView<'_>)"],
            _ => &[],
        };
        assert_eq!(skipped, want, "{}: what is left out", c.name);
        match classes.sources[c.source].root {
            "toy" => toy_part.extend(code),
            _ => alt_part.extend(code),
        }
    }
    let (functions, skipped) = classes.functions("toy", &lib, &vocab);
    assert!(skipped.is_empty(), "report and paint map");
    toy_part.extend(functions);
    for (s, code) in classes.error_codes() {
        assert_eq!(classes.sources[s].root, "toy", "the errors are toy's");
        toy_part.extend(code);
    }
    for (s, code) in classes.union_code() {
        assert_eq!(classes.sources[s].root, "toy", "Item is toy's");
        toy_part.extend(code);
    }

    let template = quote! {
        #[doc = " A number or `'auto'` as a `toy::Width`: `GEN.converters`."]
        fn width_from_js(v: &Either<u32, String>, name: &str) -> Result<toy::Width> {
            match v {
                Either::A(n) => u8::try_from(*n)
                    .map(toy::Width::Fixed)
                    .map_err(|_| Error::from_reason(format!("invalid {name}: {n}"))),
                Either::B(s) if s == "auto" => Ok(toy::Width::Auto),
                Either::B(s) => Err(Error::from_reason(format!("invalid {name}: {s:?}"))),
            }
        }
    };
    // The two `Color`s keep their name: `shared_names`.
    let surfaces = namespaced(
        &[("toy", toy_part), ("alt", alt_part)],
        &classes.shared_names(),
    );
    assert!(surfaces.report.is_empty(), "nothing is left out");
    let mirrored = surfaces.namespaced;
    let prelude = prelude();
    let code = quote! {
        #prelude
        #template
        #mirrored

        // Copy on write has no Rust counterpart to mirror: these two let the
        // test see it. The namespaced items are named by the prefix rule:
        // `ToyTally`.
        #[doc = " A second handle on the same `Tally`: shared until one of them writes."]
        #[napi(js_name = "share", namespace = "toy")]
        pub fn toy_share(t: &ToyTally) -> Result<ToyTally, String> {
            Ok(ToyTally::wrap(t.__ref()?.clone()))
        }

        #[doc = " Whether two handles still share one value."]
        #[napi(js_name = "shared", namespace = "toy")]
        pub fn toy_shared(a: &ToyTally, b: &ToyTally) -> Result<bool, String> {
            Ok(std::sync::Arc::ptr_eq(a.__ref()?, b.__ref()?))
        }
    };
    let mut file: syn::File = syn::parse2(code).expect("the toy template is valid Rust");
    // `dispose()` on every class, `[Symbol.dispose]` set at load, and the
    // `.d.ts` header declaring it (`napi.dtsHeaderFile` in package.json).
    napi_gen::dispose::slotted(&mut file);
    let (alias, header) = napi_gen::dispose::aliases(&file);
    let header_path = here.join("dts-header.d.ts");
    if fs::read_to_string(&header_path).ok().as_deref() != Some(header.as_str()) {
        fs::write(&header_path, header).unwrap();
    }
    let inspect = inspect_members(&file);
    let text = format!(
        "// @generated by build.rs from the toy crate's sources. DO NOT EDIT.\n\
         #![allow(dead_code, clippy::field_reassign_with_default)]\n{}",
        quote!(#file #alias #inspect)
    );

    let staged = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    fs::write(&staged, &text).unwrap();
    let _ = Command::new("rustfmt")
        .arg("--edition=2024")
        .arg(&staged)
        .status();
    let formatted = fs::read_to_string(&staged).unwrap();
    let dest = here.join("src/lib.rs");
    if fs::read_to_string(&dest).ok().as_deref() != Some(formatted.as_str()) {
        fs::write(&dest, formatted).unwrap();
    }
}
