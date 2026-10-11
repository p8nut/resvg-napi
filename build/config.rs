//! What this generator knows about resvg in particular.
//!
//! The other phases describe *how* to map a Rust API to napi; this file names
//! the resvg/usvg/fontdb types and members those rules do not discover on their
//! own. Each list is an API decision, not a description of upstream, and each
//! says why it exists. Wrapping another crate starts by rewriting this file.
//!
//! Not here yet: the sources and their roots (`build.rs`, `sources`), the
//! signature guards, the `Pass` table, the `classify` arms for domain types
//! (`Size`, `Rect`, `tiny_skia_path::Path`) and the tree content view in
//! `emit::wrapper_class`. Those are code shaped by usvg, not lists.

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

/// `name` through a rename table, or unchanged.
pub fn renamed<'a>(table: &[(&str, &'a str)], name: &'a str) -> &'a str {
    table
        .iter()
        .find(|(from, _)| *from == name)
        .map_or(name, |(_, to)| to)
}
