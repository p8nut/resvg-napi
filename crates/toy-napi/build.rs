//! Writes src/lib.rs from the toy crate's sources with `napi-gen`: the generic
//! generator, on a crate it was not written for. Everything specific to `toy`
//! is in `GEN` and the small template below.

use std::{env, fs, path::PathBuf, process::Command};

use napi_gen::emit::*;
use napi_gen::sources::{ModTree, parse_crate};
use napi_gen::vocab::Vocab;
use napi_gen::{Config, Shapes};
use quote::quote;

/// The toy has no transform, size, box, path data or font tag, no feature
/// gates and nothing to rename: most answers are empty.
const GEN: Config = Config {
    log_env: "TOY_NAPI_CODEGEN_LOG",
    // No u8 option field, so no table to clamp an index to.
    clamp: ("_index", "TABLE"),
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
    let (fields, skipped, _) = map_struct(&files, "Options", &vocab, 0);
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
    // constructor per class name: `toy.Counter` has to wait for the flat one
    // to go. The rest is copied.
    let (mirrored, left_out) = namespaced(
        &[("toy", quote! { #enum_code #point #ns_counter #tally })],
        &class_names(&flat),
    );
    assert_eq!(
        left_out,
        ["toy.Counter: a class named Counter exists already"],
        "only the colliding class is left out"
    );
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
