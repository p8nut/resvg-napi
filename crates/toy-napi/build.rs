//! Writes src/lib.rs from the toy crate's sources with `napi-gen`: the generic
//! generator, on a crate it was not written for, through the same
//! `napi_gen::generate::generate` the root crate calls. Everything specific to
//! `toy` is in `GEN` and the small template below; the assertions read the
//! codegen's report.

use napi_gen::{Config, Converter, Crate, Iterated, Provider, View};
use quote::quote;

/// The toy has no font tag and no feature gates: those answers are empty.
const GEN: Config = Config {
    log_env: "TOY_NAPI_CODEGEN_LOG",
    generated_from: "the toy crate's sources",
    // The crate root's items under `toy`, the `alt` module's under `alt`.
    crates: &[Crate {
        pkgs: &["toy"],
        root: "toy",
        ns: "toy",
        modules: &[("alt", "alt")],
        ..Crate::EMPTY
    }],
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
    // A `Trail` is what its `steps()` iterates: `scene.trail()` is an array
    // of `Step` unions.
    iterated: &[Iterated {
        ty: "Trail",
        via: "steps",
        item: "Step",
    }],
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
        ("toy", "copies"),
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
    ..Config::EMPTY
};

fn main() {
    napi_build::setup();
    println!("cargo::rerun-if-changed=build.rs");

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
    // Copy on write has no Rust counterpart to mirror: these two let the
    // test see it. The namespaced items are named by the prefix rule:
    // `ToyTally`.
    let tail = quote! {
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
    let report = napi_gen::generate::generate(&GEN, template, tail).report;
    // The report lines containing `part`, in report order.
    let lines = |part: &str| -> Vec<&str> {
        report
            .iter()
            .map(String::as_str)
            .filter(|l| l.contains(part))
            .collect()
    };

    // `pub use toy_dep::Shade`: another crate's enum, read as the toy's,
    // found through Cargo.lock as a workspace member.
    assert_eq!(
        lines("re-exports enum"),
        ["toy re-exports enum Shade: read as its own"],
        "Shade is the one re-exported enum"
    );
    assert_eq!(
        lines("index bound"),
        ["toy index bound for `*_index` derived from STYLES: 2"],
        "STYLES has three entries"
    );
    // The classes a borrowing member hands out a copy of: `Canvas` (owned)
    // and `Tally`; `Layer` and `Dot` (projected) are borrowed instead, when
    // reached from a projection: the `layer` of a `Frame`, a `Content`
    // holding a `Layer`.
    assert_eq!(
        lines("classes a borrowing member"),
        [
            "classes a borrowing member copies: Canvas, Tally",
            "classes a borrowing member borrows from a projection, copies otherwise: Dot, Layer",
        ],
        "the Clone classes not held by value, and those held as a projection"
    );
    // `Point` (an object `Counter::origin` returns) maps whole; `Frame` is a
    // read-only class (its `layer` is a copy of a projected class), its
    // `f64`, `&[f64]` and `i16` mapped and its `u64` unable to fit a JS
    // number. `Layer` and `Dot` are walked as data and left to their
    // projection classes.
    assert_eq!(
        lines("data types emitted"),
        ["data types emitted: Dot, Frame, Layer, Point"],
        "the data types"
    );
    assert_eq!(
        lines("Frame:")
            .into_iter()
            .chain(lines("Frame member"))
            .collect::<Vec<_>>(),
        [
            "Frame: partial mapping, emitted as read-only class Frame",
            "Frame member not exposed: serial: u64",
        ],
        "Frame maps all but serial"
    );
    assert!(lines("Point member").is_empty(), "Point maps completely");
    // The unions: `Step` (a unit variant, one carrying a plain-data class,
    // one carrying two), `Shade` (re-exported), `Content` (variants carrying
    // a class and a read-only class), `Origin` (shared bytes, a path, and a
    // tuple under its variant's own `#[cfg(unix)]`), `Label` and the error
    // `PaintError`. `Weighted::Held(Canvas, u32)` is a class beside a number,
    // an object field napi requires `Clone` of: refused, rather than a union
    // that does not compile.
    assert_eq!(
        lines("payload enums emitted"),
        ["payload enums emitted as unions: Content, Label, Origin, PaintError, Shade, Step"],
        "the unions"
    );
    assert!(
        lines("payload enum Weighted").contains(
            &"payload enum Weighted not mapped: Held carries Canvas, and Canvas is a class, \
              which can only be carried alone"
        ),
        "a class is carried alone or not at all"
    );
    // The input enums: `Tag` (`{ type, value }`, borrowed for the call) and
    // `Origin` (`{ type, bytes }` or `{ type, value }`, given). `Label` has
    // their shape, but only a method of an enum takes it, and none is
    // emitted; `alt::Hue` is in a module discovery does not walk.
    assert_eq!(
        lines("is an input enum"),
        [
            "toy::Origin is an input enum: { type, value }",
            "toy::Tag is an input enum: { type, value }",
        ],
        "the input enums JS has somewhere to pass"
    );
    // The option objects: `ReportOptions` with every field optional over
    // `Default`, `base` taken from a `toy.Tally` (a provider), `width`
    // through `width_from_js` (a converter), `style_index` clamped to
    // `STYLES` (the build fails when a bound matches no field). `finish` is
    // a function pointer, with no JS form and no provider: left out, it
    // keeps its default. `Options` and `Filter` map whole. `Counter` has no
    // `Default`.
    assert_eq!(
        lines("option object"),
        [
            "toy::ReportOptions field not in the option object: finish: fn(String)->String \
             (no provider: keeps the default)",
            "toy::Counter not an option object: no Default to fill it from",
        ],
        "what the option objects leave out"
    );
    // Each class holds its value as its shape asks; `Vec2` and `alt::Color`
    // are plain data, and two namespaces have a `Color`. What a class leaves
    // out is listed:
    // - `toy::Color::checked`: `Option<Self>` outside the constructor. `null`
    //   would have to go through napi's constructor registry, which holds
    //   one `Color` of the two.
    // - `Canvas::as_view`: a view has no JS form of its own; `paint` borrows it.
    // - `Tally`: a `Cell` over an array is no inline vector, and `alt` takes
    //   its `Hue` nowhere, so emits no input form of it.
    assert_eq!(
        lines("is a class:")
            .into_iter()
            .chain(lines("not in the class"))
            .collect::<Vec<_>>(),
        [
            "toy::Counter is a class: Shared",
            "toy::Tally is a class: SharedCow",
            "toy::Vec2 is a class: Value, plain data",
            "toy::Color is a class: Value, a JS name another namespace has",
            "toy::Canvas is a class: Owned",
            "toy::alt::Color is a class: Value, plain data, a JS name another namespace has",
            "toy::Scene is a class: Shared",
            "toy::Key is a class: Value",
            "toy::Layer is a class: Projected",
            "toy::Dot is a class: Projected",
            "toy::Tally member not in the class: first_keys (returns std::cell::Cell<[Key;2]>)",
            "toy::Tally member not in the class: hue_name (arg: &alt::Hue)",
            "toy::Color member not in the class: checked (returns Self, a class whose JS name \
             another namespace has: only its constructor and factories may)",
            "toy::Canvas member not in the class: as_view (returns CanvasView<'_>)",
        ],
        "the classes, and what each leaves out"
    );
    assert!(
        lines("function not exposed").is_empty(),
        "every function maps"
    );
    // `Album`, reached as an `Arc` (`Scene::album`): a read-only handle,
    // whose read-only classes are borrowed from its `Arc`.
    assert!(
        lines("wrapper classes generated")
            .first()
            .is_some_and(|l| l.contains("Album"))
            && lines("toy::Album").is_empty(),
        "Album maps: frames and cover"
    );
    // The constants the crate exports, in the namespace of the module that
    // exports them: `MAX_LAYERS` re-exported from a private module, `NAME`
    // in both namespaces, and two with no JS value.
    assert_eq!(
        lines("constant"),
        [
            "toy::NAME is a constant: toy.NAME",
            "toy::SCALE is a constant: toy.SCALE",
            "toy::STRICT is a constant: toy.STRICT",
            "toy::BIG constant not exposed: u64 (no JS value)",
            "toy::ORIGIN constant not exposed: Point (no JS value)",
            "toy::alt::NAME is a constant: alt.NAME",
            "toy::MAX_LAYERS is a constant: toy.MAX_LAYERS",
        ],
        "what becomes a constant"
    );
    // Nothing is left out of the surface. Each union gets a named type
    // (`toy.Step`), but `Origin`'s name is its input form's, an interface:
    // that union is spelled out instead.
    assert_eq!(
        lines("not aliased"),
        [
            "not aliased: toy.Origin: a type named Origin exists already, so its union is spelled out"
        ],
        "nothing is left out, one union is not aliased"
    );
}
