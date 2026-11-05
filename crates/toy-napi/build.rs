//! Writes src/lib.rs from the toy crate's sources with `napi-gen`: the generic
//! generator, on a crate it was not written for. Everything specific to `toy`
//! is in `GEN` and the small template below.

use std::{env, fs, path::PathBuf, process::Command};

use napi_gen::emit::*;
use napi_gen::sources::{ModTree, parse_crate};
use napi_gen::vocab::Vocab;
use napi_gen::{Config, Converter, Provider, Shapes};
use quote::quote;

/// The toy has no transform, size, box, path data or font tag, no feature
/// gates and no data or enum to rename: most answers are empty.
const GEN: Config = Config {
    log_env: "TOY_NAPI_CODEGEN_LOG",
    // `ReportOptions::style_index` indexes `STYLES` unchecked.
    index_bounded: &[("_index", "STYLES")],
    // `ReportOptions` becomes `toy.ReportOptions`. `Counter` has no
    // `Default`, so it is refused (a class with a constructor instead).
    option_objects: &["ReportOptions", "Counter"],
    // `ReportOptions::base` is supplied by a `toy.Tally`.
    providers: &[Provider {
        ty: "Arc<Tally>",
        class: "Tally",
        provide: "v.inner.clone()",
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
    matrix_fields: &[],
    size_types: &[],
    rect_types: &[],
    opt_rect_types: &[],
    path_data_types: &[],
    tag4_types: &[],
    // Never emitted: no toy type has one of these shapes.
    shapes: Shapes {
        rect: "Rect",
        size: "Size",
        matrix: "Matrix",
        path: ("Segment", "segments"),
    },
    size_new: "Size::new",
    arg_standin: None,
    content_view: None,
    hand_wired_fields: &[],
    node_types: &[],
    not_data: &[],
    hand_wrapped: &[],
    data_renames: &[],
    enum_renames: &[],
    features: &[],
    namespaces: &[("toy", "toy")],
    // The flat `Counter` gives way to `toy.Counter`.
    legacy_prefix: Some("Legacy"),
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
    let files: Vec<syn::File> = tree
        .public_view(&all, &src)
        .into_iter()
        .map(|(_, f)| f)
        .collect();
    let (modules, root) = (&tree.paths, quote!(toy));

    // The unit enum, the plain-data struct, the option struct and the handle,
    // each through the emitter the root crate uses for the same shape.
    let mut vocab = Vocab::default();
    let wanted = ["Direction".to_string()].into();
    let (enums, enum_code, _) = map_enums(&files, &wanted, modules, &root);
    vocab.enums = enums;
    vocab.objects.insert("Point".into());
    let (point, skipped, _) = object_struct(&files, "Point", &vocab, modules, &root);
    assert!(
        !point.is_empty() && skipped.is_empty(),
        "Point maps completely"
    );
    let bounds = index_bounds(&files);
    assert_eq!(bounds, [("_index", 2)], "STYLES has three entries");
    let (fields, skipped, _) = map_struct(&files, "Options", &vocab, &bounds);
    assert!(skipped.is_empty(), "every Options field maps");
    let (counter, skipped, _) =
        wrapper_class(&files, "Counter", &vocab, modules, &root, Surface::Flat);
    let skipped: Vec<_> = skipped.iter().map(ToString::to_string).collect();
    assert_eq!(skipped, ["new (no receiver)"], "Counter's methods all map");

    // The second surface, under `toy.*`: the same items again, plus `Tally`,
    // which derives Clone and so gets its `&mut self` methods through
    // `Arc::make_mut`.
    let (ns_counter, _, _) = wrapper_class(
        &files,
        "Counter",
        &vocab,
        modules,
        &root,
        Surface::Namespaced,
    );
    let (tally, skipped, _) =
        wrapper_class(&files, "Tally", &vocab, modules, &root, Surface::Namespaced);
    let skipped: Vec<_> = skipped.iter().map(ToString::to_string).collect();
    assert_eq!(skipped, ["new (no receiver)"], "Tally's methods all map");

    // The option objects: `ReportOptions` with every field optional over
    // `Default`, `base` taken from a `toy.Tally` (a provider), `width`
    // through `width_from_js` (a converter), `style_index` clamped to
    // `STYLES`. `finish` is a function pointer, with no JS form and no
    // provider: left out, it keeps its default. `Counter` has no `Default`.
    let mut options = quote!();
    for name in GEN.option_objects {
        match option_object(&files, name, &vocab, modules, &root, &bounds) {
            Some(obj) => {
                let skipped: Vec<_> = obj.skipped.iter().map(ToString::to_string).collect();
                assert_eq!(
                    skipped,
                    ["finish: fn(String)->String (no provider: keeps the default)"],
                    "{name}: only the function pointer is left out"
                );
                assert_eq!(obj.bounded, 1, "{name}: style_index is clamped");
                options.extend(obj.code);
            }
            None => assert_eq!(*name, "Counter", "{name} has Default"),
        }
    }

    let decls = fields.iter().map(|f| &f.decl);
    let assigns = fields.iter().map(|f| &f.assign);
    let flat = quote! {
        use napi::bindgen_prelude::*;
        use napi_derive::napi;

        #[doc = " What a generated class keeps alive beside its value: nothing here."]
        #[derive(Clone)]
        pub struct Doc;

        fn to_f32(v: f64) -> f32 {
            v as f32
        }

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

        #enum_code
        #point

        #[doc = " How to build a `Counter`: every field optional."]
        #[napi(object)]
        #[derive(Default)]
        pub struct Options {
            #(#decls)*
        }

        impl Options {
            fn apply(&self, o: &mut toy::Options) {
                #(#assigns)*
            }
        }

        #counter

        #[doc = " A new `Counter`."]
        #[napi]
        pub fn counter(options: Option<Options>) -> Counter {
            let mut o = toy::Options::default();
            if let Some(options) = options {
                options.apply(&mut o);
            }
            Counter::wrap(toy::Counter::new(&o), Doc)
        }
    };
    // `Counter` is a class of the first surface already, and napi keeps one
    // constructor per class name: the flat one gives way and becomes
    // `LegacyCounter` (`GEN.legacy_prefix`), so `toy.Counter` is copied with
    // the rest.
    let surfaces = namespaced(
        &[(
            "toy",
            quote! { #enum_code #point #ns_counter #tally #options },
        )],
        &flat,
    );
    assert_eq!(
        surfaces.report,
        ["legacy flat name: LegacyCounter (was Counter), beside toy.Counter"],
        "only the colliding class is renamed, and nothing is left out"
    );
    let (flat, mirrored) = (surfaces.flat, surfaces.namespaced);
    let code = quote! {
        #flat
        #mirrored

        // Hand-written factories until the generator emits them. The
        // namespaced items are named by the prefix rule: `ToyTally`, `toy_tally`.
        #[doc = " A new, empty `toy.Tally`."]
        #[napi(js_name = "tally", namespace = "toy")]
        pub fn toy_tally() -> ToyTally {
            ToyTally::wrap(toy::Tally::new(), Doc)
        }

        #[doc = " A second handle on the same `Tally`: shared until one of them writes."]
        #[napi(js_name = "share", namespace = "toy")]
        pub fn toy_share(t: &ToyTally) -> ToyTally {
            ToyTally::wrap(t.inner.clone(), Doc)
        }

        #[doc = " Whether two handles still share one value."]
        #[napi(js_name = "shared", namespace = "toy")]
        pub fn toy_shared(a: &ToyTally, b: &ToyTally) -> bool {
            std::sync::Arc::ptr_eq(&a.inner, &b.inner)
        }

        #[doc = " `toy::report`: what `ReportOptions::to_upstream` made of `options`."]
        #[napi(js_name = "report", namespace = "toy")]
        pub fn toy_report(tally: &ToyTally, options: Option<ToyReportOptions>) -> Result<String> {
            let o = options.unwrap_or_default().to_upstream()?;
            Ok(toy::report(&tally.inner, &o))
        }
    };
    let file: syn::File = syn::parse2(code).expect("the toy template is valid Rust");
    let inspect = inspect_members(&file);
    let text = format!(
        "// @generated by build.rs from the toy crate's sources. DO NOT EDIT.\n\
         #![allow(dead_code, clippy::field_reassign_with_default)]\n{}",
        quote!(#file #inspect)
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
