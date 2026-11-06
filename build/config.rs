//! What this generator knows about resvg in particular.
//!
//! The other phases describe *how* to map a Rust API to napi; this file names
//! the resvg/usvg/fontdb types and members those rules do not discover on their
//! own. Each list is an API decision, not a description of upstream, and each
//! says why it exists. Wrapping another crate starts by rewriting this file.
//!
//! That includes the crates themselves and their roots (`UPSTREAM`), the
//! index bounds, and the allowlists of classes, functions and async twins.

use napi_gen::{Config, Converter, Iterated, Provider, View};

/// One crate the binding wraps.
pub struct Upstream {
    /// The Cargo package, and a file that identifies its sources.
    pub pkg: &'static str,
    pub marker: &'static str,
    /// The path its types are reached by from the generated code.
    pub root: &'static str,
}

/// The wrapped crates, in the order a bare name is resolved against them. The
/// first is the primary: the option structs and the clamps below are read
/// from it.
pub const UPSTREAM: &[Upstream] = &[
    Upstream {
        pkg: "usvg",
        marker: "parser/options.rs",
        root: "usvg",
    },
    Upstream {
        pkg: "fontdb",
        marker: "lib.rs",
        root: "usvg::fontdb",
    },
];

/// What the generated file says it was generated from.
pub const GENERATED_FROM: &str = "the resvg/usvg/fontdb sources";

/// The path the vocabulary-only crates' types are reached by: tiny-skia
/// re-exports what of tiny-skia-path can appear (`PathSegment`), and those
/// types are `tiny_skia.*`.
pub const VOCAB_ROOT: &str = "tiny_skia";

/// Crates parsed for vocabulary only (newtypes, aliases), never wrapped.
pub const VOCAB_CRATES: &[&str] = &["tiny-skia-path", "strict-num"];

/// The option structs mapped field by field into JS objects: what
/// `usvg.Tree` parses with, and what `toString()` writes with.
pub const PARSE_OPTIONS: &str = "Options";
pub const WRITE_OPTIONS: &str = "WriteOptions";

/// What the generic phases (`napi-gen`) read: resvg's answers to its questions.
pub const GEN: Config = Config {
    log_env: crate::CODEGEN_LOG,
    // An option field indexing an upstream table without a bounds check, by its
    // name suffix, and the table: `*_precision` indexes `POW_VEC`. Clamped to the
    // table's length; the build fails if the suffix matches nothing.
    index_bounded: &[("_precision", "POW_VEC")],
    // The option structs mirrored as plain objects, `usvg.Options` and
    // `usvg.WriteOptions` (ADR 0002, Q15): every field optional, the rest from
    // usvg's `Default` -- an empty font database, Times New Roman, the default
    // resolvers. The raw API follows Rust (Q10, Q27).
    // `fontdb.Query` too, the same shape: `Database.query` takes one, its
    // `families` a list of `fontdb.Family` (`{ type, value }`).
    option_objects: &[PARSE_OPTIONS, WRITE_OPTIONS, "Query"],
    // Option fields with no JS form, supplied by a class instance (Q26, Q27).
    // `Options::fontdb` takes a `fontdb.Database`, whose `Arc` is shared, not
    // copied. The resolvers take the hand-written `guard` providers
    // (src/guard.rs): left out, usvg's default resolvers apply, which read
    // any path and decode any image, as in Rust (Q10).
    providers: &[
        Provider {
            ty: "Arc<fontdb::Database>",
            class: "Database",
            provide: "v.__ref()?.clone()",
            doc: "A `fontdb.Database`, shared with the parse rather than copied.\n\
                  Left out, the database is empty, as in Rust: text then renders\n\
                  nothing. Load fonts into a `fontdb.Database` and pass it here.",
        },
        Provider {
            ty: "ImageHrefResolver<'a>",
            class: "GuardImageHrefResolver",
            provide: "v.resolver()?",
            doc: "A `guard.ImageHrefResolver`: supplied images, files fenced to a\n\
                  directory, SVGZ and oversized images refused, misses recorded.\n\
                  Left out, usvg's default applies, as in Rust: it reads any path\n\
                  an `<image href>` names and decodes any image. Do not parse\n\
                  untrusted SVG without one. One instance per parse.",
        },
        Provider {
            ty: "FontResolver<'a>",
            class: "GuardFontResolver",
            provide: "v.resolver()?",
            doc: "A `guard.FontResolver`, recording the named families the\n\
                  database lacks. Left out, usvg's default selection applies.",
        },
        // `Options::default_size` is a `tiny_skia.Size` (Q39), which has no
        // `fromObject` (its fields are private): JS builds one with
        // `tiny_skia.Size.fromWh(width, height)`.
        Provider {
            ty: "Size",
            class: "Size",
            provide: "*v.__ref()?",
            doc: "A `tiny_skia.Size`: `tiny_skia.Size.fromWh(width, height)`.",
        },
    ],
    // `WriteOptions::indent` and `attributes_indent` are xmlwriter's `Indent`,
    // a payload enum of a crate the generator does not parse: the template's
    // `indent_from_js` reads a number of spaces, `'tabs'` or `'none'`.
    converters: &[Converter {
        ty: "Indent",
        js: "Either<u32, String>",
        ts: "number | 'tabs' | 'none'",
        convert: "indent_from_js",
    }],
    // OpenType tags (`FontVariation::tag`): four ASCII bytes, a string in JS.
    tag4_types: &["[u8;4]", "&[u8;4]"],
    // `usvg.Path.data()`. A `tiny_skia_path::Path` cannot be a class:
    // `usvg.Path` has its JS name, and napi keeps one constructor per name.
    // JS gets the segments it iterates instead, `tiny_skia_path::PathSegment`
    // as a union (`{ type: 'cubicTo', value: [p1, p2, p] }`, each point a
    // `tiny_skia.PointObject`).
    iterated: &[Iterated {
        ty: "tiny_skia_path::Path",
        via: "segments",
        item: "PathSegment",
    }],
    // The tree itself. Group, Node and Tree are what a handle exists for, and
    // following one into the data registry drags every definition type behind it --
    // `Text::flattened` and `filter::Image::root` both hand back a Group, and
    // `ImageKind` carries a Tree. Filtered both as a member and as a candidate
    // seed, because either route reaches the same place.
    node_types: &["Group", "Node", "Tree"],
    // Conversions, not data: `Stroke::to_tiny_skia` hands the caller a
    // tiny-skia value to draw with. Counting it as an unmapped member would
    // demote Stroke to a read-only class, and any object holding one -- `Path`
    // -- would then carry a field napi cannot round-trip.
    not_data: &["to_tiny_skia"],
    // Upstream Cargo features an emitted method can be gated on, and what each is
    // for this crate: `"true"` (always built) or a cfg of ours it is built under.
    // Names are global across the parsed crates (fontdb's `memmap`, usvg's
    // `text`); one listed nowhere fails the build, so a new gate upstream is a
    // decision here, not a silent miss. Must agree with Cargo.toml,
    // which drops resvg's `memmap-fonts` -- fontdb's `memmap` -- on wasm.
    features: &[
        ("std", "true"),
        ("text", "true"),
        ("fs", "true"),
        ("fontconfig", "true"),
        ("memmap", "not(target_family = \"wasm\")"),
        // tiny-skia's PNG codec; resvg turns it on, on every target.
        ("png-format", "true"),
    ],
    // One namespace per crate (ADR 0002, Q21), each type under the crate that
    // defines it. fontdb is reached through usvg's re-export, and is still
    // `fontdb` from JS; tiny-skia and resvg through resvg's.
    namespaces: &[
        ("usvg", "usvg"),
        ("usvg::fontdb", "fontdb"),
        ("tiny_skia", "tiny_skia"),
        ("svgtypes", "svgtypes"),
        ("resvg", "resvg"),
    ],
    // The classes with their constructors, statics, errors and conversions.
    // An allowlist on purpose: tiny-skia alone has forty public types, and
    // these are the ones parsing, rendering, the helpers and the tree's
    // members need. `Transform` is tiny-skia's, so `tiny_skia.Transform`
    // (Q21); `NonZeroRect` is what usvg's boxes are, and `Point` what a path
    // segment holds; `svgtypes.Color` parses CSS colours (Q33).
    classes: &[
        ("usvg", "Tree"),
        ("usvg::fontdb", "Database"),
        ("usvg::fontdb", "ID"),
        ("tiny_skia", "Pixmap"),
        ("tiny_skia", "Color"),
        ("tiny_skia", "IntSize"),
        ("tiny_skia", "Transform"),
        ("tiny_skia", "Rect"),
        ("tiny_skia", "NonZeroRect"),
        ("tiny_skia", "Size"),
        ("tiny_skia", "Point"),
        ("svgtypes", "Color"),
    ],
    // `resvg.render`, and `resvg.renderNode`, which takes a `usvg.Node`.
    functions: &[("resvg", "render"), ("resvg", "render_node")],
    // `resvg::render` draws on a `&mut PixmapMut`: JS passes the
    // `tiny_skia.Pixmap`, borrowed with `as_mut()` for the call.
    views: &[View {
        ty: "PixmapMut",
        class: "Pixmap",
        via: "as_mut",
    }],
    // The tree's nodes are borrowed from it (ADR 0002, Q22): `usvg.Group`,
    // `usvg.Path`, `usvg.Image` and `usvg.Text` hold the tree and the way back
    // to their node, and `usvg.Node` is a `{ kind, value }` union of them.
    // `tree.root()`, `group.children()` and `tree.nodeById()` hand them out.
    // The definitions owning content are reached the same way, from the
    // tree's tables or from what refers to them (`group.clipPath()`), and
    // their `root()` is a group of the same tree.
    projections: &[
        ("usvg", "Node"),
        ("usvg", "Group"),
        ("usvg", "Path"),
        ("usvg", "Image"),
        ("usvg", "Text"),
        ("usvg", "ClipPath"),
        ("usvg", "Mask"),
        ("usvg", "Pattern"),
    ],
    // `*Async` twins (ADR 0001): what is slow enough to leave the event
    // loop. Parsing, writing back, rendering, PNG encoding and loading fonts
    // from disk. A render leases its `tiny_skia.Pixmap` (Q28): it is in use
    // until the promise settles. No render-then-encode chain (Q29): two
    // round trips, which the helpers chain.
    asynchronous: &[
        ("usvg", "Tree::from_data"),
        ("usvg", "Tree::from_str"),
        ("usvg", "Tree::to_string"),
        ("resvg", "render"),
        ("resvg", "render_node"),
        ("tiny_skia", "Pixmap::encode_png"),
        ("usvg::fontdb", "Database::load_system_fonts"),
        ("usvg::fontdb", "Database::load_fonts_dir"),
    ],
};

/// Crates parsed only for the classes and functions `GEN` lists, never for
/// discovery: (the Cargo packages, read as one, and the path they are
/// reached by). tiny-skia re-exports `Transform`, `Rect` and `Size` from
/// tiny-skia-path, so the two are one source; resvg re-exports both crates.
pub const CLASS_CRATES: &[(&[&str], &str)] = &[
    (&["tiny-skia", "tiny-skia-path"], "tiny_skia"),
    (&["svgtypes"], "svgtypes"),
    (&["resvg"], "resvg"),
];
