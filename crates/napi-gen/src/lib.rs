//! Generates a napi-rs binding layer from the *actual* sources of the crates
//! it wraps: `syn` parses them, `quote` re-emits Rust. Used from a build
//! script.
//!
//! One phase per module:
//!
//!   sources  finding and parsing upstream (env override -> cargo registry)
//!   vocab    what a Rust type means on the JS side -- newtypes, aliases,
//!            `Deref` chains, payload enums, naming
//!   emit     turning those answers into tokens napi can expand
//!   cfg      which upstream methods exist on which target
//!   class    classes and free functions of the namespaced surface:
//!            constructors, statics, `Result` errors, conversions
//!   project  classes and unions over values borrowed from an owner
//!
//! What is specific to the wrapped crates is not here: the build script fills
//! a [`Config`] and hands it over with [`configure`] before any phase runs.

use std::sync::OnceLock;

/// The codegen's own report: what it derived from upstream, and what it left
/// alone with the reason. Cargo gives a build script no channel but
/// `cargo::warning`, so this competes with real warnings -- clippy's four were
/// buried under 101 lines of it. Off unless [`Config::log_env`] is set.
#[macro_export]
macro_rules! report {
    ($($arg:tt)*) => {
        if ::std::env::var_os($crate::config().log_env).is_some() {
            println!("cargo::warning={}", format!($($arg)*));
        }
    };
}

pub mod cfg;
pub mod class;
pub mod emit;
pub mod project;
pub mod sources;
pub mod vocab;

/// What the phases need to know about the wrapped crates and do not discover
/// on their own. Each list is an API decision, not a description of upstream:
/// the build script owns the values, and says why each one is there.
pub struct Config {
    /// The env var that turns [`report!`] on.
    pub log_env: &'static str,
    /// Index-bounded fields: a `u8` option field indexing an upstream table
    /// without a bounds check, by its name suffix, and the `static` table it
    /// indexes. Clamped to the table's last index (see [`emit::index_bounds`]).
    pub index_bounded: &'static [(&'static str, &'static str)],
    /// Upstream option structs mirrored as plain objects on the namespaced
    /// surface ([`emit::option_object`]): every field optional, and what is
    /// left out keeps the struct's `Default`. A struct listed without
    /// `Default` is reported and left out.
    pub option_objects: &'static [&'static str],
    /// Option fields of a type with no JS form, supplied by a class instance
    /// instead (a database, a resolver).
    pub providers: &'static [Provider],
    /// Option fields of a type the generator cannot read (a payload enum of
    /// a crate it does not parse), converted from a JS value by a template
    /// function instead.
    pub converters: &'static [Converter],
    /// A struct with these fields is a transform, mapped to `Shapes::matrix`.
    pub matrix_fields: &'static [&'static str],
    /// Upstream type spellings for the template's domain shapes.
    pub size_types: &'static [&'static str],
    pub rect_types: &'static [&'static str],
    pub opt_rect_types: &'static [&'static str],
    pub path_data_types: &'static [&'static str],
    pub tag4_types: &'static [&'static str],
    /// What the template calls those shapes: the types generated code
    /// converts them into.
    pub shapes: Shapes,
    /// The upstream size type's fallible `(width, height)` constructor, as a
    /// path from the generated file. An option field of that type is
    /// flattened into a `*_width`/`*_height` pair and rebuilt through it.
    pub size_new: &'static str,
    /// An argument type JS hands over as a class instance instead: (the
    /// upstream type, the class, and the class's method that resolves it
    /// against the receiver before the call).
    pub arg_standin: Option<(&'static str, &'static str, &'static str)>,
    /// What a handle owning content adds to its class, when the template has
    /// such a view.
    pub content_view: Option<ContentView>,
    /// Option fields the template wires by hand; every other unmappable field
    /// is dropped.
    pub hand_wired_fields: &'static [&'static str],
    /// The tree's own types: never followed as data.
    pub node_types: &'static [&'static str],
    /// Conversions, not data: members never counted as unmapped data.
    pub not_data: &'static [&'static str],
    /// Upstream handle types the template wraps by hand, with their class.
    pub hand_wrapped: &'static [(&'static str, &'static str)],
    /// Data types emitted under another name.
    pub data_renames: &'static [(&'static str, &'static str)],
    /// Enums emitted under another name.
    pub enum_renames: &'static [(&'static str, &'static str)],
    /// Upstream Cargo features an emitted method can be gated on, and what
    /// each is for the binding crate: `"true"` (always built) or a cfg it is
    /// built under. A gate listed nowhere fails the build.
    pub features: &'static [(&'static str, &'static str)],
    /// The second surface: the JS namespace each upstream source's types are
    /// mirrored into, keyed by the source's root path (`("toy", "toy")`). A
    /// type goes in the namespace of the source that defines it; a source
    /// listed nowhere gets none. See [`emit::namespaced`] for the naming rule.
    pub namespaces: &'static [(&'static str, &'static str)],
    /// What a flat class is renamed with when a namespaced copy has its JS
    /// name (`Some("Legacy")`: `ClipPath` becomes `LegacyClipPath`). napi
    /// keeps one constructor per class name, so the two cannot share it;
    /// `None` leaves the copy out instead. See [`emit::namespaced`].
    pub legacy_prefix: Option<&'static str>,
    /// Upstream types mirrored as classes of the namespaced surface by
    /// [`class::Classes`], by source root and name (`("toy", "Canvas")`):
    /// associated functions become the constructor (`new`) and statics,
    /// `FromStr` and `Display` become `fromStr` and `toString`, and a struct
    /// whose fields are all public and convertible gets `fromObject` and
    /// `toObject`. How a class holds its value is read off the type (see
    /// [`class::Holding`]). An allowlist: a type is a class only when listed.
    pub classes: &'static [(&'static str, &'static str)],
    /// Free functions mirrored into their source's namespace, by source root
    /// and name (`("toy", "paint")`). An allowlist, like `classes`.
    pub functions: &'static [(&'static str, &'static str)],
    /// Borrowed views: an argument `&mut V` is obtained from an instance of a
    /// class by one of its methods, for the call (`CanvasView` from a
    /// `Canvas` by `as_view`).
    pub views: &'static [View],
    /// Types only ever borrowed from an `Arc`-held owner, by source root and
    /// name (`("toy", "Layer")`): a struct becomes a class holding a
    /// projection (the owner and the path to the value), and an enum whose
    /// variants each carry one of those structs a union of `{ kind, value }`
    /// objects. A method returning `&T`, `Option<&T>` or `&[T]` of either,
    /// on a projected class or one behind an `Arc`, hands out projections.
    /// See [`project`].
    pub projections: &'static [(&'static str, &'static str)],
}

/// "An argument `&mut <ty>` is `<class>`'s `<via>()`, borrowed for the
/// call." JS passes the class instance.
pub struct View {
    /// The view type, by name (`CanvasView`).
    pub ty: &'static str,
    /// The class it is borrowed from, by upstream name (`Canvas`).
    pub class: &'static str,
    /// The class's `&mut self` method that returns the view (`as_view`).
    pub via: &'static str,
}

/// The template types the domain shapes become. Each has a `From` impl for
/// every upstream spelling [`Config`] lists for it.
pub struct Shapes {
    /// What a `rect_types` or `opt_rect_types` value becomes.
    pub rect: &'static str,
    /// What a `size_types` value becomes.
    pub size: &'static str,
    /// What a struct with the `matrix_fields` becomes.
    pub matrix: &'static str,
    /// What a `path_data_types` value becomes: the segment type, and the
    /// template function that returns a `Vec` of them.
    pub path: (&'static str, &'static str),
}

/// "An option field of type `ty` is supplied by an instance of `class`
/// providing it." On an option object the field takes the instance, and
/// `provide` makes the field's value out of it.
pub struct Provider {
    /// The field's upstream type, as the generator spells it (no spaces):
    /// `Arc<toy::Tally>`.
    pub ty: &'static str,
    /// The class's Rust ident in the generated code. A namespaced class is
    /// named as before [`emit::namespaced`] prefixes it, by its upstream name
    /// (`Tally`); a class the template writes, by its own ident.
    pub class: &'static str,
    /// A Rust expression of `v`, a `&` to the instance, giving the value.
    pub provide: &'static str,
    /// A paragraph added to the field's doc comment: what leaving it out
    /// means, say. Empty for none.
    pub doc: &'static str,
}

/// "An option field of type `ty` takes the JS type `js`, converted by the
/// template function `convert`": `fn(&js, field_js_name: &str) -> Result<ty>`.
pub struct Converter {
    /// The field's upstream type, as the generator spells it.
    pub ty: &'static str,
    /// The Rust type napi converts the JS value into.
    pub js: &'static str,
    /// Its TypeScript spelling.
    pub ts: &'static str,
    /// The template function, by name.
    pub convert: &'static str,
}

/// A handle whose upstream type has `pub fn <accessor.0>(&self) -> <accessor.1>`
/// owns content, and its class gets what `emit` writes, given the handle's
/// upstream path and the class.
pub struct ContentView {
    pub accessor: (&'static str, &'static str),
    pub emit: fn(&proc_macro2::TokenStream, &proc_macro2::Ident) -> proc_macro2::TokenStream,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Hands the generator its config: once, before any phase runs.
pub fn configure(config: Config) {
    assert!(
        CONFIG.set(config).is_ok(),
        "napi_gen::configure called twice"
    );
}

/// The config [`configure`] was given.
pub fn config() -> &'static Config {
    CONFIG
        .get()
        .expect("napi_gen::configure must run before the generator does")
}

/// The namespace [`Config::namespaces`] gives the source reached by `root`.
pub fn namespace_of(root: &str) -> Option<&'static str> {
    config()
        .namespaces
        .iter()
        .find(|(r, _)| *r == root)
        .map(|(_, ns)| *ns)
}

/// `name` through a rename table, or unchanged.
pub fn renamed<'a>(table: &[(&str, &'a str)], name: &'a str) -> &'a str {
    table
        .iter()
        .find(|(from, _)| *from == name)
        .map_or(name, |(_, to)| to)
}
