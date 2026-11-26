//! Writes src/lib.rs from the toy crate's sources with `napi-gen`: the generic
//! generator, on a crate it was not written for, through the same
//! `napi_gen::generate::generate` the root crate calls. Everything specific to
//! `toy` is in `GEN`, the small template below and src/hand.rs; the assertions
//! read the codegen's report.

use napi_gen::{Config, Converter, Crate, HandWritten, Iterated, Provider, Role, View};
use quote::quote;

/// Every field set, each for a toy item that needs it. No `..Config::EMPTY`:
/// a knob added to `Config` stops this compiling until it is named here, and
/// `every_field_set` fails the build while it is left empty.
const GEN: Config = Config {
    log_env: "TOY_NAPI_CODEGEN_LOG",
    generated_from: "the toy crate's sources",
    // The crate root's items under `toy`, the `alt` module's under `alt`.
    // `toy-dep` is read for its listed classes alone, under `dep`: `Grid`,
    // and the value types its members reach.
    crates: &[
        Crate {
            pkgs: &["toy"],
            root: "toy",
            ns: "toy",
            modules: &[("alt", "alt")],
            ..Crate::EMPTY
        },
        Crate {
            pkgs: &["toy-dep"],
            root: "toy_dep",
            ns: "dep",
            role: Role::Classes,
            modules: &[],
        },
    ],
    // `toy` re-exports `toy_dep`, and this crate does not depend on it:
    // the generated code reaches it by the re-export.
    uses: &["toy::toy_dep"],
    // `hand.Shout`, the provider below, written by hand.
    hand_written: &[HandWritten {
        path: "src/hand.rs",
        doc: "The toy's hand-written providers (`hand.*`): src/hand.rs.",
    }],
    // `ReportOptions::style_index` indexes `STYLES` unchecked.
    index_bounded: &[("_index", "STYLES")],
    // `Entry::names`, which `Tally::push_entry` takes from JS, may not be
    // empty: an empty one throws at the call.
    non_empty: &[("Entry", "names")],
    // `ReportOptions`, `Options` and `Filter` need no entry: `report`,
    // `Counter::new` and `Tally::find` take them by `&`, so they become
    // `toy.ReportOptions`, `toy.Options` and `toy.Filter`. `Filter` borrows
    // a list of `Tag`s, an input enum: `toy.Filter` holds `toy.Tag` objects,
    // converted for the call. `Counter` is listed for the refusal: it has no
    // `Default`, so it is reported and left out (it is a class with a
    // constructor instead).
    option_objects: &["Counter"],
    // `ReportOptions::base`, an `Arc<Tally>`, is supplied by a `toy.Tally`
    // (its `Arc` shared), and `ReportOptions::from`, a `Key`, by a `toy.Key`
    // (copied), each derived from its class: the first entry only documents
    // it. `ReportOptions::finish`, a function pointer, has no class to
    // derive one from: the hand-written `hand.Shout` supplies it.
    providers: &[
        Provider {
            ty: "Arc<Tally>",
            doc: "A `toy.Tally`, shared with the report rather than copied.",
            ..Provider::DOC
        },
        Provider {
            ty: "fn(String)->String",
            class: "HandShout",
            provide: "v.finish()?",
            doc: "A `hand.Shout`. Left out, the line is unchanged.",
        },
    ],
    // `ReportOptions::width` is a payload enum: `width_from_js` below reads
    // a number or `'auto'`.
    converters: &[Converter {
        ty: "Width",
        js: "Either<f64, String>",
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
    // `Frame::code`, four ASCII bytes: a string.
    tag4_types: &["[u8;4]"],
    // A `Layer` holds the rest of the tree, which its projection class
    // hands out: never followed as data.
    node_types: &["Layer"],
    // `Point::to_xy` converts the point: not a member it lacks, so `Point`
    // stays a plain object.
    not_data: &["to_xy"],
    // As Cargo.toml turns them on: `extra` always (`Canvas::area`), `gpu`
    // on wasm alone (`Canvas::glow`). Elsewhere `glow` is kept, and throws.
    features: &[("extra", "true"), ("gpu", "target_family = \"wasm\"")],
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
        ("toy_dep", "Grid"),
    ],
    functions: &[
        ("toy", "report"),
        ("toy", "paint"),
        ("toy", "count"),
        ("toy", "visit"),
        ("toy", "spin"),
        ("toy", "copies"),
        ("toy", "boom"),
    ],
    // Async twins, one of each shape: a static that throws
    // (`Canvas.fromTextAsync`), free functions taking a view of an owned
    // class (`paintAsync`, which leases the canvas), a union
    // (`countAsync`) and numbers (`spinAsync`, slow enough to queue
    // behind), and methods on each holding that crosses differently: an
    // owned receiver leased for `&mut` (`fillAsync`), a copy-on-write one
    // leased (`pushAsync`) and an `Arc` cloned (`joinedAsync`). `boomAsync`
    // and `Canvas.smashAsync` panic on the worker thread, the second holding
    // the lease.
    asynchronous: &[
        ("toy", "Canvas::from_text"),
        ("toy", "Canvas::fill"),
        ("toy", "Canvas::smash"),
        ("toy", "Canvas::boom"),
        ("toy", "boom"),
        ("toy", "Tally::push"),
        ("toy", "Tally::joined"),
        ("toy", "paint"),
        ("toy", "count"),
        ("toy", "spin"),
    ],
    // `paint` writes through a `CanvasView`, which JS hands over as the
    // `Canvas` it borrows. Two of its methods return one (`as_view` and
    // `last_row`): the entry says which.
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
    every_field_set();
    napi_build::setup();
    println!("cargo::rerun-if-changed=build.rs");

    let template = quote! {
        #[doc = " A number or `'auto'` as a `toy::Width`: `GEN.converters`."]
        fn width_from_js(
            v: &Either<f64, String>,
            name: &str,
        ) -> std::result::Result<toy::Width, napi_support::Thrown> {
            match v {
                Either::A(n) => napi_support::int(name, *n).map(toy::Width::Fixed),
                Either::B(s) if s == "auto" => Ok(toy::Width::Auto),
                Either::B(s) => Err(napi_support::invalid(name, format!("{s:?}"))),
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
    // The classes a borrowing member hands out a copy of: `Canvas` (owned),
    // `Tally` and `dep.Grid`; `Layer` and `Dot` (projected) are borrowed
    // instead, when reached from a projection: the `layer` of a `Frame`, a
    // `Content` holding a `Layer`.
    assert_eq!(
        lines("classes a borrowing member"),
        [
            "classes a borrowing member copies: Canvas, Grid, Tally",
            "classes a borrowing member borrows from a projection, copies otherwise: Dot, Layer",
        ],
        "the Clone classes not held by value, and those held as a projection"
    );
    // `Point` (an object `Counter::origin` returns) maps whole; `Frame` is a
    // read-only class (its `layer` is a copy of a projected class), its
    // `f64`, `&[f64]`, `i16` and `[u8; 4]` (a tag) mapped and its `u64`
    // unable to fit a JS number. `Dot` is walked as data and left to its
    // projection class; `Layer`, a node type, is not walked at all.
    // `Entry` is a read-only class too (its `key` is a class), and
    // `Tally::push_entry` takes one by value: its `EntryObject`.
    assert_eq!(
        lines("data types emitted"),
        ["data types emitted: Dot, Entry, Frame, Point"],
        "the data types"
    );
    assert_eq!(
        lines("taken by value"),
        ["toy::Entry is taken by value, as EntryObject"],
        "the data types taken by value"
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
    // `Point::to_xy` is a conversion (`GEN.not_data`), not a member it lacks.
    assert!(lines("Point member").is_empty(), "Point maps completely");
    // The unions: `Step` (a unit variant, one carrying a plain-data class,
    // one carrying two), `Shade` (re-exported), `Content` (variants carrying
    // a class and a read-only class), `Origin` (shared bytes, a path, and a
    // tuple under its variant's own `#[cfg(unix)]`), `Label`, the error
    // `PaintError`, and `Ink`, carrying an `Arc<Stamp>` as the handle class
    // `Stamp`, which nothing else returns. `Weighted::Held(Canvas, u32)` is a
    // class beside a number, an object field napi requires `Clone` of:
    // refused, rather than a union that does not compile.
    assert_eq!(
        lines("payload enums emitted"),
        ["payload enums emitted as unions: Content, Ink, Label, Origin, PaintError, Shade, Step"],
        "the unions"
    );
    // Only a real gap is reported. `Item` (a union of projections),
    // `ParseError` (thrown with its `code`) and `Tag` (an input enum) are no
    // unions, but have a JS form: not reported. `Weighted` has none.
    assert_eq!(
        lines("not mapped"),
        [
            "payload enum Weighted not mapped: Held carries Canvas, and Canvas is a class, \
             which can only be carried alone"
        ],
        "a class is carried alone or not at all, and a claimed enum is no gap"
    );
    // The same for the data types: what a payload or a member reaches but
    // another mapping claims (the unions, `f64`, the `Arc<Dot>` a handle
    // holds) is not reported; `Frame::serial`'s `u64` has no JS form at all.
    assert_eq!(
        lines("not a public struct"),
        ["object u64 skipped: not a public struct"],
        "only a type nothing maps is a skipped data type"
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
    // `Default`, `base` taken from a `toy.Tally` and `from` from a `toy.Key`
    // (providers, derived), `finish`, a function pointer with no JS form,
    // from the hand-written `hand.Shout` (a provider, listed), `width`
    // through `width_from_js` (a converter), `style_index` clamped to
    // `STYLES` (the build fails when a bound matches no field). `Options`
    // and `Filter` map whole. `Counter` has no `Default`.
    assert_eq!(
        lines("option object"),
        ["toy::Counter not an option object: no Default to fill it from"],
        "what the option objects leave out"
    );
    // Each class holds its value as its shape asks; `Vec2` and `alt::Color`
    // are plain data, and two namespaces have a `Color`. What a class leaves
    // out is listed:
    // - `toy::Color::checked`: `Option<Self>` outside the constructor. `null`
    //   would have to go through napi's constructor registry, which holds
    //   one `Color` of the two.
    // - `Canvas::as_view` and `last_row`: a view has no JS form of its own;
    //   `paint` borrows the first (`GEN.views`).
    // - `Tally`: a `Cell` over an array is no inline vector, and `alt` takes
    //   its `Hue` nowhere, so emits no input form of it.
    // - `dep.Grid::cells`: a slice of a value class is not followed (an
    //   array of instances), and `stamp` takes a `Token`, `Copy` but with
    //   nothing to call, so no class.
    // `Vec2::from_array` and `to_array` are left out but not listed: they
    // convert plain data to a type nothing maps, and `fromObject` and
    // `toObject` already carry it. `Vec2::sum` and `shift_all` take a slice
    // of it, an array of `Vec2Object`s.
    // `dep.Area`, `dep.Cell` and `dep.Mark` are listed nowhere: `Grid`'s
    // members reach the first two, `Cell::mark` the third (the fixpoint).
    assert_eq!(
        lines("reached by"),
        [
            "toy_dep::Area reached by toy_dep::Grid::bounds: a class, held by value",
            "toy_dep::Cell reached by toy_dep::Grid::cell: a class, held by value",
            "toy_dep::Mark reached by toy_dep::Cell::mark: a class, held by value",
        ],
        "the value classes the listed members reach"
    );
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
            "toy_dep::Grid is a class: SharedCow",
            "toy_dep::Area is a class: Value",
            "toy_dep::Cell is a class: Value",
            "toy_dep::Mark is a class: Value, plain data",
            "toy::Layer is a class: Projected",
            "toy::Dot is a class: Projected",
            "toy::Tally member not in the class: first_keys (returns std::cell::Cell<[Key;2]>)",
            "toy::Tally member not in the class: hue_name (arg: &alt::Hue)",
            "toy::Color member not in the class: checked (returns Self, a class whose JS name \
             another namespace has: only its constructor and factories may)",
            "toy::Canvas member not in the class: as_view (returns CanvasView<'_>)",
            "toy::Canvas member not in the class: last_row (returns CanvasView<'_>)",
            "toy_dep::Grid member not in the class: cells (returns &[Cell])",
            "toy_dep::Grid member not in the class: stamp (arg: Token)",
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
    // in both namespaces, `HALF` a newtype over `f32`, and two with no JS
    // value. `toy_dep`, read for its classes alone, has its `CELL_BYTES`
    // (a `usize`) and `CELL_FULL` (its type an alias of its own) mirrored
    // all the same, but the associated constant of a type of it that is no
    // class (`Token::SIZE`) reported, never emitted. The associated
    // constants left out: a `u64` (on a class and on a number newtype), and
    // `Self` on a class not held by value, which could not hand out a copy
    // per read.
    assert_eq!(
        lines("constant"),
        [
            "toy_dep constants not exposed: Token::SIZE (read only for its listed classes and \
             functions)",
            "toy::Color::BITS constant not exposed: u64 (no JS value)",
            "toy::Canvas::EMPTY constant not exposed: Canvas is not held by value, so no copy \
             per read",
            "toy::NAME is a constant: toy.NAME",
            "toy::SCALE is a constant: toy.SCALE",
            "toy::STRICT is a constant: toy.STRICT",
            "toy::BIG constant not exposed: u64 (no JS value)",
            "toy::ORIGIN constant not exposed: Point (no JS value)",
            "toy::HALF is a constant: toy.HALF",
            "toy::alt::NAME is a constant: alt.NAME",
            "toy::MAX_LAYERS is a constant: toy.MAX_LAYERS",
            "toy::MinLen::HUGE constant not exposed: u64 (no JS value)",
            "toy_dep::CELL_BYTES is a constant: dep.CELL_BYTES",
            "toy_dep::CELL_FULL is a constant: dep.CELL_FULL",
        ],
        "what becomes a constant"
    );
    // The associated constants with a JS form, static read-only: on a class,
    // a number, a string, and `Self` held by value (a fresh instance per
    // read, the class's name shared or not, of a listed class or of one a
    // listed class reaches); on a number newtype, which has no class, an
    // object of its name holding numbers.
    assert_eq!(
        lines("is a static"),
        [
            "toy::Color::BLACK is a static: toy.Color.BLACK",
            "toy::Color::CHANNELS is a static: toy.Color.CHANNELS",
            "toy::Color::KIND is a static: toy.Color.KIND",
            "toy::Key::LAST is a static: toy.Key.LAST",
            "toy_dep::Area::UNIT is a static: dep.Area.UNIT",
            "toy::MinLen::NONE is a static: toy.MinLen.NONE",
            "toy::MinLen::WORD is a static: toy.MinLen.WORD",
            "toy::MinLen::MAX is a static: toy.MinLen.MAX",
        ],
        "what becomes a static"
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

/// The toy is the proof each `Config` knob is generic, so it sets every one:
/// the knobs `GEN` leaves empty fail the build.
fn every_field_set() {
    let empty: Vec<&str> = [
        ("log_env", GEN.log_env.is_empty()),
        ("generated_from", GEN.generated_from.is_empty()),
        ("crates", GEN.crates.is_empty()),
        ("uses", GEN.uses.is_empty()),
        ("hand_written", GEN.hand_written.is_empty()),
        ("index_bounded", GEN.index_bounded.is_empty()),
        ("non_empty", GEN.non_empty.is_empty()),
        ("option_objects", GEN.option_objects.is_empty()),
        // A listed class, not only a doc on a derived one.
        (
            "providers",
            GEN.providers.iter().all(|p| p.class.is_empty()),
        ),
        ("converters", GEN.converters.is_empty()),
        ("tag4_types", GEN.tag4_types.is_empty()),
        ("iterated", GEN.iterated.is_empty()),
        ("node_types", GEN.node_types.is_empty()),
        ("not_data", GEN.not_data.is_empty()),
        ("features", GEN.features.is_empty()),
        ("classes", GEN.classes.is_empty()),
        ("functions", GEN.functions.is_empty()),
        ("views", GEN.views.is_empty()),
        ("projections", GEN.projections.is_empty()),
        ("asynchronous", GEN.asynchronous.is_empty()),
    ]
    .into_iter()
    .filter_map(|(name, empty)| empty.then_some(name))
    .collect();
    assert!(empty.is_empty(), "GEN leaves Config knobs unset: {empty:?}");
}
