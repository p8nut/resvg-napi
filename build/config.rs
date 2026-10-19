//! What this generator knows about resvg in particular.
//!
//! The other phases describe *how* to map a Rust API to napi; this file names
//! the resvg/usvg/fontdb types and members those rules do not discover on their
//! own. Each list is an API decision, not a description of upstream, and each
//! says why it exists. Wrapping another crate starts by rewriting this file.
//!
//! That includes the crates themselves and their roots (`UPSTREAM`), the
//! signature guards, the wrapped impl blocks (`PASSES`), the clamp, and the
//! upstream spellings of the template's domain shapes. The names the generated
//! code expects the template to define (`BBox`, `Doc`, `IntoJs`, `Signal`, ...)
//! are the contract between the two halves: `TEMPLATE_CONTRACT`, checked
//! against the template at every build.

/// One crate the binding wraps.
pub struct Upstream {
    /// The Cargo package, and a file that identifies its sources.
    pub pkg: &'static str,
    pub marker: &'static str,
    /// The path its types are reached by from the generated code.
    pub root: &'static str,
    /// How the report names it.
    pub label: &'static str,
}

/// The wrapped crates, in the order a bare name is resolved against them. The
/// first is the primary: the option structs, the guards and the clamps below
/// are read from it.
pub const UPSTREAM: &[Upstream] = &[
    Upstream {
        pkg: "usvg",
        marker: "parser/options.rs",
        root: "usvg",
        label: "usvg",
    },
    Upstream {
        pkg: "fontdb",
        marker: "lib.rs",
        root: "usvg::fontdb",
        label: "usvg::fontdb",
    },
];

/// What the generated file says it was generated from.
pub const GENERATED_FROM: &str = "the resvg/usvg/fontdb sources";

/// The path the vocabulary-only crates' types are reached by: usvg re-exports
/// what of tiny-skia-path can appear.
pub const VOCAB_ROOT: &str = "usvg";

/// The renderer, its crate root, and the entry point whose signature the
/// template calls: `resvg::render(&Tree, Transform, &mut PixmapMut)`.
pub const RENDERER: (&str, &str) = ("resvg", "lib.rs");
pub const RENDER_FN: (&str, &[&str]) = (
    "render",
    &[
        "&usvg::Tree",
        "tiny_skia::Transform",
        "&muttiny_skia::PixmapMut",
    ],
);

/// The document type, and the methods the template calls on it by hand.
pub const TREE: (&str, &[&str]) = ("Tree", &["from_data", "to_string"]);

/// Upstream structs the template fills in by hand, with the fields it sets.
pub const HAND_FILLED: &[(&str, &[&str])] = &[
    ("ImageHrefResolver", &["resolve_data", "resolve_string"]),
    ("FontResolver", &["select_font", "select_fallback"]),
];

/// An option field indexing an upstream table without a bounds check, by its
/// name suffix, and the table: `*_precision` indexes `POW_VEC`. Clamped to the
/// table's length; the build fails if the suffix matches nothing.
pub const CLAMP: (&str, &str) = ("_precision", "POW_VEC");

/// A struct with these fields is a transform (tiny-skia's), mapped to `Matrix`.
pub const MATRIX_FIELDS: &[&str] = &["sx", "kx", "ky", "sy", "tx", "ty"];

/// Upstream type spellings for the template's domain shapes.
pub const SIZE_TYPES: &[&str] = &["Size", "usvg::Size"];
pub const RECT_TYPES: &[&str] = &["Rect", "NonZeroRect"];
pub const OPT_RECT_TYPES: &[&str] = &["Option<NonZeroRect>"];
pub const PATH_DATA_TYPES: &[&str] = &["tiny_skia_path::Path", "&tiny_skia_path::Path"];
pub const TAG4_TYPES: &[&str] = &["[u8;4]", "&[u8;4]"];

/// An argument type JS hands over as a class instance instead: a fontdb face
/// ID arrives as the `FontFace` it names, resolved by `id_in`.
pub const ARG_STANDIN: (&str, &str) = ("ID", "FontFace");

/// One wrapped `impl` block: which upstream type's methods land on which
/// class, called on what. `receiver` and `prologue` are Rust source.
pub struct PassSpec {
    pub label: &'static str,
    pub target: &'static str,
    /// The `Upstream::label` whose files hold the impl.
    pub source: &'static str,
    pub ty: &'static str,
    pub receiver: &'static str,
    pub skip: &'static [&'static str],
    pub prologue: Option<&'static str>,
    /// The receiver may not exist: every wrapper returns `Result<Option<_>>`.
    pub optional: bool,
}

/// The wrapped impl blocks, in emission order. Group runs on SvgNode after
/// Node: what both define (`id`, the boxes) is taken by Node, and stays
/// non-null on every node.
pub const PASSES: &[PassSpec] = &[
    PassSpec {
        label: "fontdb::Database",
        target: "FontDatabase",
        source: "usvg::fontdb",
        ty: "Database",
        receiver: "self.inner",
        skip: &[],
        prologue: None,
        optional: false,
    },
    PassSpec {
        label: "usvg::Tree",
        target: "Resvg",
        source: "usvg",
        ty: "Tree",
        receiver: "self.tree",
        skip: &[],
        prologue: None,
        optional: false,
    },
    PassSpec {
        label: "usvg::Group",
        target: "Resvg",
        source: "usvg",
        ty: "Group",
        receiver: "self.tree.root()",
        skip: &["isolate", "should_isolate", "id"],
        prologue: None,
        optional: false,
    },
    PassSpec {
        label: "usvg::Node",
        target: "SvgNode",
        source: "usvg",
        ty: "Node",
        receiver: "__n",
        skip: &["subroots"],
        prologue: Some("let __n = self.node()?;"),
        optional: false,
    },
    PassSpec {
        label: "usvg::Group (on SvgNode)",
        target: "SvgNode",
        source: "usvg",
        ty: "Group",
        receiver: "__g",
        skip: &["filters_bounding_box"],
        prologue: Some("let usvg::Node::Group(__g) = self.node()? else { return Ok(None); };"),
        optional: true,
    },
];

/// Methods a pass skips because the API serves them another way: (pass, the
/// skip as reported, by what).
pub const COVERED_ELSEWHERE: &[(&str, &str, &str)] = &[(
    "usvg::Node",
    "subroots (arg: F)",
    // A visitor over the groups a node refers to. Each one is already a
    // handle of its own, so a callback API would add nothing.
    "clipPath / mask / filters / imageChildren / paint ids",
)];

/// What the generated code names and the template must define.
pub const TEMPLATE_CONTRACT: &[&str] = &[
    "BBox",
    "Dimensions",
    "Matrix",
    "PathSegment",
    "path_segments",
    "Doc",
    "HasRoot",
    "SvgNode",
    "IntoJs",
    "Signal",
    "to_f32",
];

/// Crates parsed for vocabulary only (newtypes, aliases), never wrapped.
pub const VOCAB_CRATES: &[&str] = &["tiny-skia-path", "strict-num"];

/// The option structs mapped field by field into JS objects: what `Resvg`
/// parses with, and what `toString()` writes with.
pub const PARSE_OPTIONS: &str = "Options";
pub const WRITE_OPTIONS: &str = "WriteOptions";

/// Option fields the template wires by hand (opaque class, resolver hooks,
/// xmlwriter's `Indent`); every other unmappable field is dropped.
pub const HAND_WIRED_FIELDS: &[&str] = &[
    "fontdb",
    "image_href_resolver",
    "font_resolver",
    "indent",
    "attributes_indent",
];

/// Upstream names the template exposes under a *different* one. A rule cannot
/// find these -- the rename is the decision -- so they are declared, and
/// checked: a stale entry fails the build instead of rotting in a log line.
pub const RENAMED: &[(&str, &str)] = &[
    ("from_data", "new"),
    ("from_str", "new"),
    ("from_xmltree", "new"),
    ("from_data_nested", "new"),
    ("node_by_id", "node"),
    // `to_string` in Rust would collide with `Display::to_string`
    ("to_string", "to_svg_string"),
    ("root", "children"),
    ("with_face_data", "face_data"),
];

/// Class names the template defines itself; a generated handle class must not
/// take one.
pub const RESERVED: &[&str] = &[
    "Resvg",
    "SvgNode",
    "BBox",
    "Matrix",
    "Dimensions",
    "RawImage",
    "RenderOptions",
    "RenderParams",
];

/// The tree itself. Group, Node and Tree are what a handle exists for, and
/// following one into the data registry drags every definition type behind it --
/// `Text::flattened` and `filter::Image::root` both hand back a Group, and
/// `ImageKind` carries a Tree. Filtered both as a member and as a candidate
/// seed, because either route reaches the same place.
pub const NODE_TYPES: &[&str] = &["Group", "Node", "Tree"];

/// Conversions, not data: `Stroke::to_tiny_skia` hands the caller a
/// tiny-skia value to draw with. Counting it as an unmapped member would
/// demote Stroke to a read-only class, and any object holding one -- `Path`
/// -- would then carry a field napi cannot round-trip.
pub const NOT_DATA: &[&str] = &["to_tiny_skia"];

/// Upstream handle types the template wraps by hand, with the class name
/// they get. Handle discovery skips them; `wrapper_ident` names them.
pub const HAND_WRAPPED: &[(&str, &str)] = &[("fontdb::Database", "FontDatabase")];

/// Data types emitted under another name: `FaceInfo` reads as `FontFace`.
pub const DATA_RENAMES: &[(&str, &str)] = &[("FaceInfo", "FontFace")];

/// Enums emitted under another name. `Style` alone says nothing next to usvg's
/// own `FontStyle`, and the two would sit side by side in the exports with
/// identical variants: one is what a text run asked for, the other what a
/// loaded face is.
pub const ENUM_RENAMES: &[(&str, &str)] = &[("Style", "FontFaceStyle")];

/// Upstream Cargo features an emitted method can be gated on, and what each is
/// for this crate: `"true"` (always built), `"false"` (never), or a cfg of ours
/// it is built under. Names are global across the parsed crates (fontdb's
/// `memmap`, usvg's `text`); one listed nowhere fails the build, so a new gate
/// upstream is a decision here, not a silent miss. Must agree with Cargo.toml,
/// which drops resvg's `memmap-fonts` -- fontdb's `memmap` -- on wasm.
pub const FEATURES: &[(&str, &str)] = &[
    ("std", "true"),
    ("text", "true"),
    ("fs", "true"),
    ("fontconfig", "true"),
    ("memmap", "not(target_family = \"wasm\")"),
];

/// `name` through a rename table, or unchanged.
pub fn renamed<'a>(table: &[(&str, &'a str)], name: &'a str) -> &'a str {
    table
        .iter()
        .find(|(from, _)| *from == name)
        .map_or(name, |(_, to)| to)
}
