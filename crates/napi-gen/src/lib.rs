//! Generates a napi-rs binding layer from the *actual* sources of the crates
//! it wraps: `syn` parses them, `quote` re-emits Rust. Used from a build
//! script.
//!
//! One phase per module:
//!
//!   sources  finding and parsing upstream (env override -> workspace
//!            member -> cargo registry)
//!   shape    what a type is built of, read once from `syn`
//!   vocab    what a Rust type means on the JS side -- newtypes, aliases,
//!            `Deref` chains, payload enums, naming
//!   emit     turning those answers into tokens napi can expand
//!   cfg      which upstream methods exist on which target
//!   class    classes and free functions of the namespaced surface:
//!            constructors, statics, `Result` errors, conversions
//!   project  classes and unions over values borrowed from an owner
//!   dispose  `dispose()` and `[Symbol.dispose]` for every class
//!   statics  associated constants, as static read-only properties
//!   twin     `*Async` twins, on a worker thread, with leases
//!   generate the whole pipeline, in order, writing the binding crate's
//!            `src/lib.rs`: what a build script calls
//!
//! What is specific to the wrapped crates is not here: the build script fills
//! a [`Config`] and hands it to the phases that read it -- inside the
//! [`vocab::Vocab`] and the [`class::Classes`] most of them already take, or as
//! an argument to the few that run before either exists. No global: two
//! configs can live in one process (the unit tests use that).

/// The codegen's own report: what it derived from upstream, and what it left
/// alone with the reason. Cargo gives a build script no channel but
/// `cargo::warning`, so this competes with real warnings -- clippy's four were
/// buried under 101 lines of it. Off unless the [`Config::log_env`] of the
/// config given first is set: `report!(config, "{x} skipped")`.
#[macro_export]
macro_rules! report {
    ($config:expr, $($arg:tt)*) => {
        if ::std::env::var_os($config.log_env).is_some() {
            println!("cargo::warning={}", format!($($arg)*));
        }
    };
}

pub mod cfg;
pub mod class;
pub mod dispose;
pub mod emit;
pub mod generate;
pub mod project;
pub mod shape;
pub mod sources;
pub mod statics;
pub mod twin;
pub mod vocab;

/// What the phases need to know about the wrapped crates and do not discover
/// on their own. Each list is an API decision, not a description of upstream:
/// the build script owns the values, and says why each one is there.
///
/// `Copy`: every list is `&'static`, so a phase holds its own copy rather
/// than a borrow. [`Config::EMPTY`] (and `Default`) is the empty config:
/// nothing listed.
#[derive(Clone, Copy)]
pub struct Config {
    /// The env var that turns [`report!`] on.
    pub log_env: &'static str,
    /// What the generated file says it was generated from, in its first line.
    pub generated_from: &'static str,
    /// The crates the binding reads, in the order a bare name is resolved
    /// against them: each crate's packages, the path it is reached by from
    /// the generated code, its JS namespace (see [`emit::namespaced`] for
    /// the naming rule), and what it is read for (see [`Crate`]). The first
    /// [`Role::Discover`] crate is the primary: the index bounds are read
    /// from it.
    pub crates: &'static [Crate],
    /// `use` paths the generated file starts with, before the hand-written
    /// modules: how it reaches the crates' roots when they are not its own
    /// dependencies' names (`a::{b, c}`).
    pub uses: &'static [&'static str],
    /// Modules of the binding crate written by hand (see [`HandWritten`]).
    pub hand_written: &'static [HandWritten],
    /// Index-bounded fields: a `u8` option field indexing an upstream table
    /// without a bounds check, by its name suffix, and the `static` table it
    /// indexes. Clamped to the table's last index (see [`emit::index_bounds`]).
    pub index_bounded: &'static [(&'static str, &'static str)],
    /// Upstream option structs mirrored as plain objects on the namespaced
    /// surface ([`emit::option_object`]): every field optional, and what is
    /// left out keeps the struct's `Default`. Beside the ones derived: a
    /// struct with `Default` that a listed member of a discovered crate takes
    /// by `&` needs no entry. A struct listed without `Default` is reported
    /// and left out.
    pub option_objects: &'static [&'static str],
    /// Option fields of a type with no JS form, supplied by an instance of a
    /// class the template writes instead (a resolver). One of a listed class
    /// needs no entry: an `Arc<T>` field of a class held behind an `Arc` is
    /// supplied by an instance of it, the `Arc` shared, and a `T` field of a
    /// class held by value and not plain data by an instance of it, copied.
    /// An entry naming no
    /// `class` ([`Provider::DOC`]) only documents such a field.
    pub providers: &'static [Provider],
    /// Option fields of a type the generator cannot read (a payload enum of
    /// a crate it does not parse), converted from a JS value by a template
    /// function instead.
    pub converters: &'static [Converter],
    /// An `[u8; 4]` spelling: an OpenType tag, a string in JS.
    pub tag4_types: &'static [&'static str],
    /// Types JS sees as the list one of their methods iterates (see
    /// [`Iterated`]).
    pub iterated: &'static [Iterated],
    /// Types never followed as data, by name (`Layer`): no data type is
    /// derived for one, whether a member returns it, a payload carries it or
    /// it is projected, and a data type's member of one is left out. For the
    /// nodes of a projected tree: each has its projection class already, and
    /// following one only drags every type behind it through the data walk
    /// (and the report). One a listed class copies is still that class.
    pub node_types: &'static [&'static str],
    /// Members that are conversions, not data, by name (`to_xy`): a data
    /// type's member so named is neither mapped nor counted as one it lacks,
    /// so it does not demote the type to a read-only class.
    pub not_data: &'static [&'static str],
    /// Upstream Cargo features an emitted method can be gated on, and what
    /// each is for the binding crate: `"true"` (always built) or a cfg it is
    /// built under. A gate listed nowhere fails the build.
    pub features: &'static [(&'static str, &'static str)],
    /// Upstream types mirrored as classes of the namespaced surface by
    /// [`class::Classes`], by source root and name (`("toy", "Canvas")`):
    /// associated functions become the constructor (`new`) and statics,
    /// `FromStr` and `Display` become `fromStr` and `toString`, and a struct
    /// whose fields are all public and convertible gets `fromObject` and
    /// `toObject`. How a class holds its value is read off the type (see
    /// [`class::Holding`]). An allowlist: a type is a class only when listed,
    /// or when it is a `Copy` struct with a public inherent function that a
    /// class of a [`Role::Classes`] crate takes or returns, by value, `&`,
    /// `Option` or `Result` (not in a slice): a class held by value with no
    /// entry, to a fixpoint, and reported.
    pub classes: &'static [(&'static str, &'static str)],
    /// Free functions mirrored into their source's namespace, by source root
    /// and name (`("toy", "paint")`). An allowlist, like `classes`.
    pub functions: &'static [(&'static str, &'static str)],
    /// Borrowed views: an argument `&mut V` is obtained from an instance of a
    /// class by one of its methods, for the call (`CanvasView` from a
    /// `Canvas` by `as_view`). Derived with no entry when exactly one `&mut
    /// self` method of a listed class, taking nothing else, returns `V<'_>`:
    /// an entry settles two.
    pub views: &'static [View],
    /// Types only ever borrowed from an `Arc`-held owner, by source root and
    /// name (`("toy", "Layer")`): a struct becomes a class holding a
    /// projection (the owner and the path to the value), and an enum whose
    /// variants each carry one of those structs a union of `{ kind, value }`
    /// objects. A method returning `&T`, `Option<&T>` or `&[T]` of either,
    /// on a projected class or one behind an `Arc`, hands out projections.
    /// See [`project`].
    pub projections: &'static [(&'static str, &'static str)],
    /// The members that get an async twin, by source root and name: a
    /// method or a static as `Type::name` (`("toy", "Canvas::fill")`), a
    /// free function by its name (`("toy", "paint")`). The twin is
    /// `<name>Async(...args, signal?)`, returning a promise: its arguments
    /// are converted on the main thread, the call runs on a worker thread.
    /// A value it takes by reference and holds by value (an owned class, a
    /// `&mut` one) is leased to the call: see `napi_support::Slot::lease`.
    /// Each entry must name a member of a listed class or function that has
    /// a JS form, and a twin of it must be possible, or the build fails.
    pub asynchronous: &'static [(&'static str, &'static str)],
}

/// One crate the binding reads ([`Config::crates`]).
#[derive(Clone, Copy)]
pub struct Crate {
    /// The Cargo packages, read as one source (a crate and the one it
    /// re-exports, say). A [`Role::Discover`] crate has exactly one.
    pub pkgs: &'static [&'static str],
    /// The path its types are reached by from the generated code (`a`, or
    /// `a::b` for a crate another one re-exports).
    pub root: &'static str,
    /// The JS namespace its types are mirrored into (ADR 0002, Q21): each
    /// type goes in the namespace of the crate that defines it.
    pub ns: &'static str,
    pub role: Role,
    /// Modules of a [`Role::Discover`] crate with a namespace of their own,
    /// by module path (`("alt", "alt")`): their types are a source of their
    /// own (`a::alt`), as another crate's are. Every other module's types
    /// are in the crate's namespace.
    pub modules: &'static [(&'static str, &'static str)],
}

/// What a crate of [`Config::crates`] is read for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Wrapped and walked: its enums, the data types its members reach, its
    /// handles, option objects, input enums and constants are found on their
    /// own, and its listed classes and functions are emitted.
    Discover,
    /// Read only for the classes and functions [`Config::classes`] and
    /// [`Config::functions`] list for it, and the value types their classes'
    /// members reach (see [`Config::classes`]): nothing else of it is
    /// discovered, so nothing else would give those a JS form.
    Classes,
    /// Read only for vocabulary (newtypes, aliases, payload enums), never
    /// wrapped: a dependency whose types the others re-export.
    Vocabulary,
}

impl Crate {
    /// Nothing named: `Crate { pkgs: &["a"], root: "a", ns: "a", ..Crate::EMPTY }`,
    /// a [`Role::Discover`] crate with no module namespace.
    pub const EMPTY: Crate = Crate {
        pkgs: &[],
        root: "",
        ns: "",
        role: Role::Discover,
        modules: &[],
    };
}

/// A module of the binding crate written by hand, beside the generated file:
/// the generated file declares it (`mod <stem>;`), imports the classes of it
/// that [`Config::providers`] name, and gives its classes `[Symbol.dispose]`
/// and `util.inspect` as it does its own.
pub struct HandWritten {
    /// Its file, from the binding crate's manifest directory (`src/x.rs`).
    pub path: &'static str,
    /// The `mod` declaration's doc comment.
    pub doc: &'static str,
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

/// "A value of type `ty` is the list its method `via` iterates, each item an
/// `item`": JS gets an `Array` of the items, each mapped as `item` maps. For a
/// type with no JS form of its own whose content an iterator hands out (a
/// stream of payload-enum segments, say).
pub struct Iterated {
    /// The type, by the path naming it (`a::B`): no `&`, no arguments.
    pub ty: &'static str,
    /// Its `&self` method returning the iterator.
    pub via: &'static str,
    /// The iterator's item type, by name.
    pub item: &'static str,
}

/// "An option field of type `ty` is supplied by an instance of `class`
/// providing it." On an option object the field takes the instance, and
/// `provide` makes the field's value out of it. With `class` empty, the
/// provider is the one derived from a listed class (see
/// [`Config::providers`]), and `doc` is added to it.
pub struct Provider {
    /// The field's upstream type, as the generator spells it (no spaces):
    /// `Arc<toy::Tally>`.
    pub ty: &'static str,
    /// The class's Rust ident in the generated code. A namespaced class is
    /// named as before [`emit::namespaced`] prefixes it, by its upstream name
    /// (`Tally`); a class the template writes, by its own ident.
    pub class: &'static str,
    /// A Rust expression of `v`, a `&` to the instance, giving the value. It
    /// may `?` a `Result<_, String>`: `v.__ref()?` is a generated class's
    /// value, which throws `Disposed` once `dispose()` emptied it.
    pub provide: &'static str,
    /// A paragraph added to the field's doc comment: what leaving it out
    /// means, say. Empty for none.
    pub doc: &'static str,
}

impl Provider {
    /// A doc for a derived provider: `Provider { ty: "T", doc: "...",
    /// ..Provider::DOC }`. The build fails when no listed class provides `ty`.
    pub const DOC: Provider = Provider {
        ty: "",
        class: "",
        provide: "",
        doc: "",
    };
}

/// "An option field of type `ty` takes the JS type `js`, converted by the
/// template function `convert`": `fn(&js, field_js_name: &str) -> Result<ty, napi_support::Thrown>`,
/// a bad value an `InvalidArg` (`napi_support::invalid`, `napi_support::int`).
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

impl Config {
    /// Nothing listed: `Config { log_env: "X", ..Config::EMPTY }`. What
    /// `Default` gives, usable in a `const`.
    pub const EMPTY: Config = Config {
        log_env: "",
        generated_from: "",
        crates: &[],
        uses: &[],
        hand_written: &[],
        index_bounded: &[],
        option_objects: &[],
        providers: &[],
        converters: &[],
        tag4_types: &[],
        iterated: &[],
        node_types: &[],
        not_data: &[],
        features: &[],
        classes: &[],
        functions: &[],
        views: &[],
        projections: &[],
        asynchronous: &[],
    };

    /// The namespace [`Config::crates`] gives the source reached by `root`:
    /// a crate's, or a module's of its own (`a::alt`). The first match.
    pub fn namespace_of(&self, root: &str) -> Option<&'static str> {
        self.crates.iter().find_map(|c| {
            if c.root == root {
                return Some(c.ns);
            }
            let m = root.strip_prefix(c.root)?.strip_prefix("::")?;
            c.modules.iter().find(|(n, _)| *n == m).map(|(_, ns)| *ns)
        })
    }

    /// The crates of one role, in their order.
    pub fn crates_of(&self, role: Role) -> impl Iterator<Item = &'static Crate> {
        self.crates.iter().filter(move |c| c.role == role)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::EMPTY
    }
}
