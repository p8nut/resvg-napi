//! What this generator knows about resvg in particular.
//!
//! The other phases describe *how* to map a Rust API to napi; this file names
//! the resvg/usvg/fontdb types and members those rules do not discover on their
//! own. Each list is an API decision, not a description of upstream, and each
//! says why it exists. Wrapping another crate starts by rewriting this file.
//!
//! That includes the crates themselves and their roots (`UPSTREAM`), the
//! wrapped impl blocks (`PASSES`), the index bounds, and the upstream spellings of
//! the template's domain shapes.

use napi_gen::{Config, Converter, Provider, Shapes, View};

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

/// The path the vocabulary-only crates' types are reached by: usvg re-exports
/// what of tiny-skia-path can appear.
pub const VOCAB_ROOT: &str = "usvg";

/// One wrapped `impl` block: which upstream type's methods land on which
/// class, called on what. `receiver` is Rust source.
pub struct PassSpec {
    pub label: &'static str,
    pub target: &'static str,
    /// The `Upstream::root` whose files hold the impl.
    pub source: &'static str,
    pub ty: &'static str,
    pub receiver: &'static str,
    pub skip: &'static [&'static str],
}

/// The wrapped impl blocks, in emission order. The tree's nodes and
/// definitions are `usvg.*` projections (`GEN.projections`): the document's
/// own passes leave out what would hand one out.
pub const PASSES: &[PassSpec] = &[
    PassSpec {
        label: "fontdb::Database",
        target: "FontDatabase",
        source: "usvg::fontdb",
        ty: "Database",
        receiver: "self.inner",
        skip: &[],
    },
    PassSpec {
        label: "usvg::Tree",
        target: "Resvg",
        source: "usvg",
        ty: "Tree",
        receiver: "self.tree",
        // The definition tables: `usvg.Tree`'s, as projections.
        skip: &[
            "linear_gradients",
            "radial_gradients",
            "patterns",
            "clip_paths",
            "masks",
            "filters",
        ],
    },
    PassSpec {
        label: "usvg::Group",
        target: "Resvg",
        source: "usvg",
        ty: "Group",
        receiver: "self.tree.root()",
        // `filters` hands out definitions, which are `usvg.Tree`'s.
        skip: &["isolate", "should_isolate", "id", "filters"],
    },
];

/// Crates parsed for vocabulary only (newtypes, aliases), never wrapped.
pub const VOCAB_CRATES: &[&str] = &["tiny-skia-path", "strict-num"];

/// The option structs mapped field by field into JS objects: what `Resvg`
/// parses with, and what `toString()` writes with.
pub const PARSE_OPTIONS: &str = "Options";
pub const WRITE_OPTIONS: &str = "WriteOptions";

/// Upstream names the template exposes under a *different* one. A rule cannot
/// find these -- the rename is the decision -- so they are declared, and
/// checked: a stale entry fails the build instead of rotting in a log line.
pub const RENAMED: &[(&str, &str)] = &[
    ("from_data", "new"),
    ("from_str", "new"),
    ("from_xmltree", "new"),
    ("from_data_nested", "new"),
    // `to_string` in Rust would collide with `Display::to_string`
    ("to_string", "to_svg_string"),
    ("with_face_data", "face_data"),
];

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
    option_objects: &[PARSE_OPTIONS, WRITE_OPTIONS],
    // Option fields with no JS form, supplied by a class instance (Q26, Q27).
    // `Options::fontdb` takes a `fontdb.Database`, whose `Arc` is shared, not
    // copied. The resolvers (`ImageHrefResolver`, `FontResolver`) get their
    // provider classes with the `guard` module in step 10 of the migration;
    // until then the fields are left out and usvg's default resolvers apply.
    providers: &[Provider {
        ty: "Arc<fontdb::Database>",
        class: "Database",
        provide: "v.inner.clone()",
        doc: "A `fontdb.Database`, shared with the parse rather than copied.\n\
              Left out, the database is empty, as in Rust: text then renders\n\
              nothing. Load fonts into a `fontdb.Database` and pass it here.",
    }],
    // `WriteOptions::indent` and `attributes_indent` are xmlwriter's `Indent`,
    // a payload enum of a crate the generator does not parse: the template's
    // `indent_from_js` reads a number of spaces, `'tabs'` or `'none'`.
    converters: &[Converter {
        ty: "Indent",
        js: "Either<u32, String>",
        ts: "number | 'tabs' | 'none'",
        convert: "indent_from_js",
    }],
    // A struct with these fields is a transform (tiny-skia's), mapped to `Matrix`.
    matrix_fields: &["sx", "kx", "ky", "sy", "tx", "ty"],
    // Upstream type spellings for the template's domain shapes.
    size_types: &["Size", "usvg::Size"],
    rect_types: &["Rect", "NonZeroRect"],
    opt_rect_types: &["Option<NonZeroRect>"],
    path_data_types: &["tiny_skia_path::Path", "&tiny_skia_path::Path"],
    tag4_types: &["[u8;4]", "&[u8;4]"],
    // The template's types for those shapes, each with a `From` per spelling.
    shapes: Shapes {
        rect: "BBox",
        size: "Dimensions",
        matrix: "Matrix",
        path: ("PathSegment", "path_segments"),
    },
    // `Options::default_size` is flattened into a width/height pair, rebuilt
    // through this. It refuses a zero or negative side, and the field keeps its
    // default then.
    size_new: "usvg::Size::from_wh",
    // An argument type JS hands over as a class instance instead: a fontdb face
    // ID arrives as the `FontFace` it names, resolved by `id_in`. fontdb IDs are
    // slotmap keys, so another database's face could name a slot here, and
    // `removeFace` would delete it.
    arg_standin: Some(("ID", "FontFace", "id_in")),
    // Option fields the template wires by hand (opaque class, resolver hooks,
    // xmlwriter's `Indent`); every other unmappable field is dropped.
    hand_wired_fields: &[
        "fontdb",
        "image_href_resolver",
        "font_resolver",
        "indent",
        "attributes_indent",
    ],
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
    // Upstream handle types the template wraps by hand, with the class name
    // they get. Handle discovery skips them; `wrapper_ident` names them.
    hand_wrapped: &[("fontdb::Database", "FontDatabase")],
    // Data types emitted under another name: `FaceInfo` reads as `FontFace`.
    data_renames: &[("FaceInfo", "FontFace")],
    // Enums emitted under another name. `Style` alone says nothing next to usvg's
    // own `FontStyle`, and the two would sit side by side in the exports with
    // identical variants: one is what a text run asked for, the other what a
    // loaded face is.
    enum_renames: &[("Style", "FontFaceStyle")],
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
    // The second surface (ADR 0002, Q21): one namespace per crate, each type
    // under the crate that defines it. fontdb is reached through usvg's
    // re-export, and is still `fontdb` from JS; tiny-skia and resvg through
    // resvg's. The flat exports stay beside it until the migration removes
    // them.
    namespaces: &[
        ("usvg", "usvg"),
        ("usvg::fontdb", "fontdb"),
        ("tiny_skia", "tiny_skia"),
        ("svgtypes", "svgtypes"),
        ("resvg", "resvg"),
    ],
    // The classes with their constructors, statics, errors and conversions
    // (step 6 of the migration). An allowlist on purpose: tiny-skia alone has
    // forty public types, and these are the ones parsing, rendering and the
    // helpers need. `Transform` is tiny-skia's, so `tiny_skia.Transform`
    // (Q21), and `svgtypes.Color` parses CSS colours (Q33).
    classes: &[
        ("usvg", "Tree"),
        ("usvg::fontdb", "Database"),
        ("tiny_skia", "Pixmap"),
        ("tiny_skia", "Color"),
        ("tiny_skia", "IntSize"),
        ("tiny_skia", "Transform"),
        ("tiny_skia", "Rect"),
        ("tiny_skia", "Size"),
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
