//! The API this crate presents, written by hand.
//!
//! Everything else the generator produces describes upstream: type aliases,
//! newtypes, `Deref` chains, enum variants and struct fields are all read from
//! the sources. This file is the other half -- the decisions about what the
//! JavaScript surface looks like -- and it is deliberately the only place they
//! live.
//!
//! `Fragments` is what the derived half hands over. Reading this file tells you
//! what the binding offers; reading `emit.rs` tells you how upstream was turned
//! into the pieces it splices in.

use proc_macro2::TokenStream;
use quote::quote;

/// The fragments the passes produce. Grouped so `template` keeps a short
/// signature: these are the same on both of its calls, while the three method
/// lists are not.
pub struct Fragments<'a> {
    pub enums: TokenStream,
    pub object_code: TokenStream,
    pub decls: Vec<&'a TokenStream>,
    pub assigns: Vec<&'a TokenStream>,
    pub write_decls: Vec<&'a TokenStream>,
    pub write_assigns: Vec<&'a TokenStream>,
}

/// The hand-written half of the binding: everything the passes cannot derive.
///
/// Called twice. Once with empty method lists, so `template_fns` can read the
/// method names it defines off the template itself -- nothing lists them by
/// hand -- and once for real.
///
/// NOTE: no inner attributes (`#![..]` / `//!`) in the body: the file is pulled
/// in with `include!`, which only accepts items.
pub fn template(
    f: &Fragments,
    fontdb_methods: &TokenStream,
    tree_methods: &TokenStream,
    group_methods: &TokenStream,
) -> TokenStream {
    // Destructured by name so the body below is unchanged from when it was a
    // closure capturing these from `main`.
    let Fragments {
        enums,
        object_code,
        decls,
        assigns,
        write_decls,
        write_assigns,
    } = f;
    quote! {
        use napi::bindgen_prelude::*;
        use napi_derive::napi;
        use resvg::{tiny_skia, usvg};
        use napi_support::fs::read_regular;
        use napi_support::logs::Logs;
        use napi_support::{Signal, contained};

        #enums

        #[doc = " Mirror of `usvg::Options`. Every field is optional; omitted fields"]
        #[doc = " keep the usvg default."]
        #[napi(object)]
        #[derive(Default, Clone)]
        pub struct RenderOptions {
            #(#decls)*
            #[doc = " The directory disk reads may not leave. Default: `resourcesDir`."]
            #[doc = ""]
            #[doc = " Hrefs still resolve against `resourcesDir`; this only widens"]
            #[doc = " what they may reach, so a template beside a shared assets folder"]
            #[doc = " can use `../assets/logo.png`. Not a usvg option: usvg reads any"]
            #[doc = " path, and this binding confines it."]
            pub resources_root: Option<String>,
        }

        #[doc = " System fonts are expensive to enumerate, so do it once per process."]
        fn default_fontdb() -> std::sync::Arc<usvg::fontdb::Database> {
            static DB: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
                std::sync::OnceLock::new();
            DB.get_or_init(|| {
                let mut db = usvg::fontdb::Database::new();
                db.load_system_fonts();
                std::sync::Arc::new(db)
            })
            .clone()
        }

        impl RenderOptions {
            fn to_usvg(
                &self,
                fonts: std::sync::Arc<usvg::fontdb::Database>,
            ) -> usvg::Options<'static> {
                let mut o = usvg::Options::default();
                #(#assigns)*
                o.fontdb = fonts;
                o
            }
        }

        type ImageMap = std::collections::HashMap<String, std::sync::Arc<Vec<u8>>>;

        #[doc = " The bytes of an SVG handed over as text or as a Uint8Array (or Buffer)."]
        fn svg_bytes(svg: Either<String, Uint8Array>) -> Vec<u8> {
            match svg {
                Either::A(text) => text.into_bytes(),
                Either::B(data) => data.to_vec(),
            }
        }

        #[doc = " JS `{ href: Buffer }` -> the map the href resolver reads, copied"]
        #[doc = " off the JS heap so a worker thread can hold it."]
        fn image_map(images: Option<std::collections::HashMap<String, Uint8Array>>) -> ImageMap {
            images
                .unwrap_or_default()
                .into_iter()
                .map(|(href, buf)| (href, std::sync::Arc::new(buf.to_vec())))
                .collect()
        }

        #[doc = " The Send half of a result, and its JS half."]
        #[doc = ""]
        #[doc = " `Buffer` holds a reference into the JS heap, so it is not `Send` and"]
        #[doc = " cannot be built on a worker thread. Every async twin therefore computes"]
        #[doc = " one of these and converts on the main thread, in `Task::resolve`."]
        pub trait IntoJs {
            type Js;
            fn into_js(self) -> Self::Js;
        }
        impl IntoJs for Vec<u8> {
            type Js = Buffer;
            fn into_js(self) -> Buffer {
                Buffer::from(self)
            }
        }
        impl IntoJs for String {
            type Js = String;
            fn into_js(self) -> String {
                self
            }
        }
        impl IntoJs for (u32, u32, Vec<u8>) {
            type Js = RawImage;
            fn into_js(self) -> RawImage {
                let (width, height, data) = self;
                RawImage {
                    width,
                    height,
                    data: Buffer::from(data),
                }
            }
        }

        #[doc = " Hrefs no resolver could satisfy during the last parse."]
        #[derive(Default, Clone)]
        struct Misses(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

        impl Misses {
            #[doc = " Records `name`, once: a `<use>` of an image resolves it again."]
            #[doc = " Poison-tolerant: this mutex only accumulates diagnostics, so a"]
            #[doc = " panic elsewhere must not take the render down with it."]
            fn note(&self, name: &str) {
                let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
                if !seen.iter().any(|s| s == name) {
                    seen.push(name.to_string());
                }
            }

            #[doc = " Drains what was recorded."]
            fn take(&self) -> std::sync::Arc<[String]> {
                std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner())).into()
            }
        }

        #[doc = " `usvg::ImageHrefResolver` demands `Send + Sync`, so a JS callback cannot"]
        #[doc = " live in it. Buffers are handed over up front instead, and whatever stays"]
        #[doc = " unresolved is reported back to JS for a second pass."]
        fn href_resolver(
            images: std::sync::Arc<ImageMap>,
            misses: Misses,
            root: Option<std::path::PathBuf>,
        ) -> usvg::ImageHrefResolver<'static> {
            let sniff = checked(usvg::ImageHrefResolver::default_data_resolver());
            usvg::ImageHrefResolver {
                // Also what a nested SVG's own images go through: usvg forwards
                // `resolve_data` into the sub-document's options.
                resolve_data: checked(usvg::ImageHrefResolver::default_data_resolver()),
                resolve_string: Box::new(move |href: &str, opts: &usvg::Options| {
                    // 1. buffer supplied by JS -- let usvg sniff the format itself.
                    // One it cannot use (refused, or not an image at all) is a
                    // miss like any other, not an image that silently vanishes.
                    if let Some(data) = images.get(href)
                        && let Some(kind) = sniff("text/plain", data.clone(), opts)
                    {
                        return Some(kind);
                    }
                    // 2. a file under `resourcesDir`, and nowhere else. usvg's
                    // default reads any absolute path, and a relative one off
                    // the working directory, so an untrusted document could
                    // pull local files into its pixels or `toString()` output,
                    // or exhaust memory reading `/dev/zero`.
                    // Read here rather than through usvg's string resolver,
                    // which re-opens the path by name after the checks. Its
                    // format rule is kept: SVG by extension, magic otherwise.
                    if let Some(path) = under_resources_dir(href, opts, root.as_deref())
                        && let Some(data) = read_regular(&path)
                    {
                        let svg = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
                        let mime = if svg { "image/svg+xml" } else { "text/plain" };
                        if let Some(kind) = sniff(mime, std::sync::Arc::new(data), opts) {
                            return Some(kind);
                        }
                    }
                    // 3. give up, but tell JS about it
                    misses.note(href);
                    None
                }),
            }
        }

        #[doc = " Largest raster image accepted, by the size its header declares: 2^28"]
        #[doc = " px. resvg decodes at render time with no limit of its own, so a 1 KB"]
        #[doc = " JPEG declaring 65535x65535 would ask for 16 GiB there."]
        const MAX_IMAGE_PIXELS: u64 = 1 << 28;

        #[doc = " `resolve` behind the two checks usvg and resvg leave out: gzip data"]
        #[doc = " (SVGZ, unsupported, see Cargo.toml) is refused, and so is a raster"]
        #[doc = " image whose header declares more than `MAX_IMAGE_PIXELS`. Refusals"]
        #[doc = " are logged."]
        fn checked(resolve: usvg::ImageHrefDataResolverFn<'static>) -> usvg::ImageHrefDataResolverFn<'static> {
            Box::new(move |mime: &str, data: std::sync::Arc<Vec<u8>>, opts: &usvg::Options| {
                if data.starts_with(&[0x1f, 0x8b]) {
                    log::warn!("{}: image refused, SVGZ is not supported", env!("CARGO_PKG_NAME"));
                    return None;
                }
                // Fail closed: anything a decoder would treat as raster -- by
                // its mime or by the magic usvg itself sniffs -- needs a size
                // read off its header, or it is not drawn.
                let raster = matches!(mime, "image/jpg" | "image/jpeg" | "image/png" | "image/gif" | "image/webp")
                    || matches!(
                        imagesize::image_type(&data),
                        Ok(imagesize::ImageType::Png | imagesize::ImageType::Jpeg | imagesize::ImageType::Gif | imagesize::ImageType::Webp)
                    );
                if raster {
                    let px = imagesize::blob_size(&data)
                        .map(|s| (s.width as u64).saturating_mul(s.height as u64));
                    match px {
                        Ok(px) if px <= MAX_IMAGE_PIXELS => {}
                        Ok(_) | Err(_) => {
                            log::warn!(
                                "{}: image refused, {} exceeds {MAX_IMAGE_PIXELS} px or has no readable size",
                                env!("CARGO_PKG_NAME"),
                                px.map_or("header".into(), |px| format!("{px} px")),
                            );
                            return None;
                        }
                    }
                }
                resolve(mime, data, opts)
            })
        }

        #[doc = " `href` resolved against `resourcesDir`, if it lands inside `root`"]
        #[doc = " (default: `resourcesDir` itself). Canonicalised, so `..` and"]
        #[doc = " symlinks cannot step outside it. What the path names is checked"]
        #[doc = " by `read_regular`, on the open descriptor."]
        fn under_resources_dir(
            href: &str,
            opts: &usvg::Options,
            root: Option<&std::path::Path>,
        ) -> Option<std::path::PathBuf> {
            let dir = opts.resources_dir.as_deref()?;
            let root = root.unwrap_or(dir).canonicalize().ok()?;
            let path = dir.join(href).canonicalize().ok()?;
            path.starts_with(&root).then_some(path)
        }


        impl FontFace {
            #[doc = " This face's ID in `db`, or one that matches nothing."]
            #[doc = ""]
            #[doc = " IDs are slotmap keys, so a face from another database can name"]
            #[doc = " a live slot of this one. The face found there must be the same"]
            #[doc = " face; a copy of the database (`fontdb()`) keeps the slots and"]
            #[doc = " still matches."]
            fn id_in(&self, db: &usvg::fontdb::Database) -> usvg::fontdb::ID {
                let me = &self.inner;
                match db.face(me.id) {
                    Some(f) if f.post_script_name == me.post_script_name && f.index == me.index => me.id,
                    _ => usvg::fontdb::ID::dummy(),
                }
            }
        }

        #[doc = " Opaque wrapper over `fontdb::Database` (memory-mapped faces, no JSON form)."]
        #[napi]
        pub struct FontDatabase {
            inner: SharedDb,
        }

        #[doc = " A database shared with every parse that used it, copied on the"]
        #[doc = " first write. A plain `Database` was deep-copied -- every `FaceInfo`"]
        #[doc = " -- by each parse, `parseAsync` and `fontdb()` call. Through"]
        #[doc = " `DerefMut` the generated `&mut self` methods keep working unchanged."]
        struct SharedDb(std::sync::Arc<usvg::fontdb::Database>);

        impl std::ops::Deref for SharedDb {
            type Target = usvg::fontdb::Database;
            fn deref(&self) -> &usvg::fontdb::Database {
                &self.0
            }
        }

        impl std::ops::DerefMut for SharedDb {
            fn deref_mut(&mut self) -> &mut usvg::fontdb::Database {
                std::sync::Arc::make_mut(&mut self.0)
            }
        }

        impl FontDatabase {
            #[doc = " Snapshot of a shared database, e.g. the one a parse resolved."]
            fn wrap(inner: std::sync::Arc<usvg::fontdb::Database>) -> Self {
                Self { inner: SharedDb(inner) }
            }
        }

        #[napi]
        impl FontDatabase {
            #[doc = " Which face this database would pick for a CSS font request."]
            #[doc = ""]
            #[doc = " Decision, not a derivation: `fontdb::Query` borrows a `&[Family]`"]
            #[doc = " whose `Name` variant holds a `&str`, so it cannot be rebuilt from a"]
            #[doc = " JS object. The request is taken apart into plain arguments instead;"]
            #[doc = " `serif`, `sans-serif`, `cursive`, `fantasy` and `monospace` are"]
            #[doc = " understood as the generic families."]
            #[napi]
            pub fn query(
                &self,
                families: Vec<String>,
                weight: Option<u32>,
                italic: Option<bool>,
            ) -> Option<FontFace> {
                let names: Vec<usvg::fontdb::Family> =
                    families.iter().map(|f| family_of(f)).collect();
                let query = usvg::fontdb::Query {
                    families: &names,
                    // CSS weights are 1..=1000; `as u16` turned 70000 into 4464.
                    weight: usvg::fontdb::Weight(
                        u16::try_from(weight.unwrap_or(400).clamp(1, 1000)).unwrap_or(400),
                    ),
                    style: if italic.unwrap_or(false) {
                        usvg::fontdb::Style::Italic
                    } else {
                        usvg::fontdb::Style::Normal
                    },
                    ..Default::default()
                };
                let id = self.inner.query(&query)?;
                self.inner.face(id).cloned().map(FontFace::wrap)
            }

            #[doc = " Raw bytes of a face, by PostScript name (see `faces()`)."]
            #[doc = ""]
            #[doc = " Decision, not a derivation: upstream exposes this through"]
            #[doc = " `with_face_data<P: FnOnce(&[u8], u32) -> T>` plus an opaque `ID`."]
            #[doc = " Neither a closure nor that key crosses to JS, so the lookup key is"]
            #[doc = " the PostScript name and the bytes are copied out."]
            #[napi]
            pub fn face_data(&self, post_script_name: String) -> Option<Buffer> {
                let id = self
                    .inner
                    .faces()
                    .find(|f| f.post_script_name == post_script_name)
                    .map(|f| f.id)?;
                self.inner
                    .with_face_data(id, |data, _index| Buffer::from(data.to_vec()))
            }

            #[doc = " The name a family resolves to: a generic keyword (`serif`,"]
            #[doc = " `sans-serif`, `cursive`, `fantasy`, `monospace`) becomes the family"]
            #[doc = " this database maps it to, any other name comes back as is."]
            #[napi]
            pub fn family_name(&self, family: String) -> String {
                self.inner.family_name(&family_of(&family)).to_string()
            }

            #[doc = " Where a face was loaded from, and its index in that file or"]
            #[doc = " buffer. `null` for a face this database does not hold."]
            #[napi]
            pub fn face_source(&self, face: &FontFace) -> Option<FaceSource> {
                let (source, index) = self.inner.face_source(face.id_in(&self.inner))?;
                let (kind, path) = match source {
                    usvg::fontdb::Source::Binary(_) => ("binary", None),
                    usvg::fontdb::Source::File(path) => ("file", Some(path)),
                    // fontdb has no SharedFile without its `memmap` feature,
                    // which Cargo.toml leaves off on wasm (WASI has no mmap).
                    #[cfg(not(target_family = "wasm"))]
                    usvg::fontdb::Source::SharedFile(path, _) => ("sharedFile", Some(path)),
                };
                Some(FaceSource {
                    kind: kind.to_string(),
                    path: path.map(|p| p.to_string_lossy().into_owned()),
                    index,
                })
            }

            #[napi(constructor)]
            pub fn new() -> Self {
                Self { inner: SharedDb(std::sync::Arc::new(usvg::fontdb::Database::new())) }
            }
            #fontdb_methods
        }

        #[doc = " A CSS family name as fontdb reads it: the five generic keywords,"]
        #[doc = " or a family name."]
        fn family_of(name: &str) -> usvg::fontdb::Family<'_> {
            match name {
                "serif" => usvg::fontdb::Family::Serif,
                "sans-serif" => usvg::fontdb::Family::SansSerif,
                "cursive" => usvg::fontdb::Family::Cursive,
                "fantasy" => usvg::fontdb::Family::Fantasy,
                "monospace" => usvg::fontdb::Family::Monospace,
                other => usvg::fontdb::Family::Name(other),
            }
        }

        #[doc = " `fontdb::Source` with the face index `faceSource` pairs it with."]
        #[doc = " The bytes of a `binary` or `sharedFile` source are not copied"]
        #[doc = " here; `faceData` reads them."]
        #[napi(object)]
        pub struct FaceSource {
            #[napi(ts_type = "'binary' | 'file' | 'sharedFile'")]
            pub kind: String,
            #[doc = " The file, for `file` and `sharedFile`."]
            pub path: Option<String>,
            #[doc = " Index of the face in a collection (`.ttc`), 0 otherwise."]
            pub index: u32,
        }

        #[doc = " Mirror of `usvg::WriteOptions`, for `Resvg.toString()`. Every field is"]
        #[doc = " optional; omitted fields keep the usvg default."]
        #[napi(object)]
        #[derive(Default, Clone)]
        pub struct WriteOptions {
            #(#write_decls)*
            #[doc = " Indentation of nested elements: a number of spaces (0-255),"]
            #[doc = " `'tabs'`, or `'none'` for everything on one line. Default: 4."]
            #[napi(ts_type = "number | 'tabs' | 'none'")]
            pub indent: Option<Either<u32, String>>,
            #[doc = " Indentation of attributes, which then go one per line. Same"]
            #[doc = " values as `indent`. Default: `'none'`, on the element's line."]
            #[napi(ts_type = "number | 'tabs' | 'none'")]
            pub attributes_indent: Option<Either<u32, String>>,
        }

        impl WriteOptions {
            fn to_usvg(&self) -> Result<usvg::WriteOptions> {
                let mut o = usvg::WriteOptions::default();
                #(#write_assigns)*
                o.indent = indent(&self.indent, o.indent, "indent")?;
                o.attributes_indent =
                    indent(&self.attributes_indent, o.attributes_indent, "attributesIndent")?;
                Ok(o)
            }
        }

        #[doc = " `usvg::Indent` is xmlwriter's, a crate the generator does not read."]
        fn indent(
            v: &Option<Either<u32, String>>,
            fallback: usvg::Indent,
            name: &str,
        ) -> Result<usvg::Indent> {
            match v {
                None => Ok(fallback),
                Some(v) => indent_from_js(v, name),
            }
        }

        #[doc = " A number of spaces (0-255), `'tabs'` or `'none'` as a `usvg::Indent`:"]
        #[doc = " `config::GEN.converters`, for `usvg.WriteOptions` too."]
        fn indent_from_js(v: &Either<u32, String>, name: &str) -> Result<usvg::Indent> {
            match v {
                Either::A(n) => u8::try_from(*n)
                    .map(usvg::Indent::Spaces)
                    .map_err(|_| Error::from_reason(format!("invalid {name}: {n}"))),
                Either::B(s) => match s.as_str() {
                    "tabs" => Ok(usvg::Indent::Tabs),
                    "none" => Ok(usvg::Indent::None),
                    _ => Err(Error::from_reason(format!("invalid {name}: {s:?}"))),
                },
            }
        }

        #[doc = " Output size / scaling of one render pass."]
        #[napi(object)]
        #[derive(Default, Clone)]
        pub struct RenderParams {
            #[doc = " Uniform scale factor. Default: 1."]
            pub scale: Option<f64>,
            #[doc = " Target width in px, rounded to the nearest; keeps the aspect"]
            #[doc = " ratio and overrides `scale`."]
            pub width: Option<f64>,
            #[doc = " Target height in px, rounded to the nearest; keeps the aspect"]
            #[doc = " ratio and overrides `scale`."]
            pub height: Option<f64>,
            #[doc = " Background colour: any CSS3 colour string, e.g. `#eee`, `teal`,"]
            #[doc = " `rgba(255, 0, 0, .5)`. Default: transparent."]
            pub background: Option<String>,
            #[doc = " Render only this rectangle, in SVG user units. `scale` / `width` /"]
            #[doc = " `height` then size the crop, not the full viewport. Feed it"]
            #[doc = " `absLayerBoundingBox()` to trim the document to its content."]
            pub crop: Option<BBox>,
            #[doc = " Refuse a canvas of more than this many pixels (4 bytes each)."]
            #[doc = " Default: 2^26, an 8192x8192 canvas or 256 MiB. Without a cap a"]
            #[doc = " document declaring `width=\"200000\"` aborts the whole process"]
            #[doc = " on the failed allocation, which no try/catch can recover."]
            #[doc = ""]
            #[doc = " It bounds the canvas only: a group with a filter, opacity, mask or"]
            #[doc = " clip path renders through a layer of its own, which resvg lets grow"]
            #[doc = " to 5x the canvas each way, measured in the group's own coordinates:"]
            #[doc = " a rotated or skewed group gets the box around that, larger still."]
            #[doc = " Raise it knowing a render can then allocate many times this cap."]
            pub max_pixels: Option<f64>,
        }

        #[doc = " An affine transform. Field names and order are tiny-skia's."]
        #[doc = ""]
        #[doc = " They are *not* the order of SVG's `matrix(a b c d e f)`, which"]
        #[doc = " takes `sx ky kx sy tx ty` -- upstream says so itself: \"we are"]
        #[doc = " using column-major-column-vector matrix notation, therefore it's"]
        #[doc = " ky-kx, not kx-ky\". Reading these positionally into a `matrix()`"]
        #[doc = " string mirrors the transform, silently. Name the fields:"]
        #[doc = " `matrix(${m.sx} ${m.ky} ${m.kx} ${m.sy} ${m.tx} ${m.ty})`."]
        #[napi(object)]
        #[derive(Clone, Copy)]
        pub struct Matrix {
            pub sx: f64,
            pub kx: f64,
            pub ky: f64,
            pub sy: f64,
            pub tx: f64,
            pub ty: f64,
        }

        impl From<usvg::Transform> for Matrix {
            fn from(t: usvg::Transform) -> Self {
                Self {
                    sx: t.sx.into(), kx: t.kx.into(), ky: t.ky.into(),
                    sy: t.sy.into(), tx: t.tx.into(), ty: t.ty.into(),
                }
            }
        }

        #[doc = " A width/height pair in SVG user units."]
        #[napi(object)]
        #[derive(Clone, Copy)]
        pub struct Dimensions {
            pub width: f64,
            pub height: f64,
        }

        impl From<usvg::Size> for Dimensions {
            fn from(s: usvg::Size) -> Self {
                Self { width: s.width().into(), height: s.height().into() }
            }
        }

        #[doc = " A rectangle in SVG user units."]
        #[napi(object)]
        #[derive(Clone, Copy)]
        pub struct BBox {
            pub x: f64,
            pub y: f64,
            pub width: f64,
            pub height: f64,
        }

        impl From<usvg::Rect> for BBox {
            fn from(r: usvg::Rect) -> Self {
                Self {
                    x: r.x().into(),
                    y: r.y().into(),
                    width: r.width().into(),
                    height: r.height().into(),
                }
            }
        }

        impl From<usvg::NonZeroRect> for BBox {
            fn from(r: usvg::NonZeroRect) -> Self {
                Self {
                    x: r.x().into(),
                    y: r.y().into(),
                    width: r.width().into(),
                    height: r.height().into(),
                }
            }
        }

        #[doc = " Un-premultiplied RGBA8 pixels, row-major, no padding."]
        #[napi(object)]
        pub struct RawImage {
            pub width: u32,
            pub height: u32,
            pub data: Buffer,
        }

        #[doc = " Same trick for fonts: the resolver is `Send + Sync` too, so instead of"]
        #[doc = " calling into JS we wrap the default selector and note every named family"]
        #[doc = " the database does not carry -- i.e. text that silently fell back."]
        fn font_resolver(misses: Misses) -> usvg::FontResolver<'static> {
            let select = usvg::FontResolver::default_font_selector();
            usvg::FontResolver {
                select_font: Box::new(
                    move |font: &usvg::Font,
                          db: &mut std::sync::Arc<usvg::fontdb::Database>| {
                        for family in font.families() {
                            let usvg::FontFamily::Named(name) = family else {
                                continue; // serif/sans-serif/... always map to something
                            };
                            // ponytail: name presence over `faces()`, weight and style
                            // ignored. Switch to `Database::query` if that granularity
                            // ever matters.
                            let known = db.faces().any(|f| {
                                f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
                            });
                            if !known {
                                misses.note(name);
                            }
                        }
                        select(font, db)
                    },
                ),
                select_fallback: usvg::FontResolver::default_fallback_selector(),
            }
        }

        #object_code

        #[doc = " Starts collecting what usvg and resvg report."]
        #[doc = ""]
        #[doc = " `level` is `off`, `error`, `warn`, `info`, `debug` or `trace`. Safe to"]
        #[doc = " call repeatedly: the logger is installed once, the level always applies."]
        // The six levels are checked at runtime and were declared as a bare
        // `string`, so a typo was a thrown error rather than a red squiggle.
        #[napi(ts_args_type = "level: 'off' | 'error' | 'warn' | 'info' | 'debug' | 'trace'")]
        pub fn set_log_level(level: String) -> Result<()> {
            napi_support::logs::set_level(&level)
        }

        #[doc = " Drains the messages collected since the last call, from every"]
        #[doc = " document and thread. Concurrent renders interleave here; a"]
        #[doc = " document's own `takeLogs()` holds only its messages."]
        #[napi]
        pub fn take_logs() -> Vec<String> {
            napi_support::logs::take()
        }

        #[doc = " The strict policy drops `FaceInfo::families` -- it is a"]
        #[doc = " `Vec<(String, Language)>`, and no rule maps a tuple with a language"]
        #[doc = " tag. But fontdb queries by *family*, not by PostScript name, so"]
        #[doc = " without this a caller cannot find the string to put in `fontFamily`."]
        #[doc = " Decision: expose the names, drop the language tags."]
        #[napi]
        impl FontFace {
            #[doc = " Family names of this face, English (US) first when present."]
            #[napi(getter)]
            pub fn families(&self) -> Vec<String> {
                self.inner.families.iter().map(|(name, _)| name.clone()).collect()
            }
        }

        #[doc = " A parsed tree with the image and font hrefs it could not resolve."]
        type Parsed = (usvg::Tree, std::sync::Arc<[String]>, std::sync::Arc<[String]>);

        #[doc = " usvg's own name for a parse failure, carried as `error.code`."]
        #[napi(string_enum)]
        pub enum ParseErrorCode {
            NotAnUtf8Str,
            SvgzFeatureNotEnabled,
            MalformedGZip,
            ElementsLimitReached,
            InvalidSize,
            ParsingFailed,
        }

        impl From<&usvg::Error> for ParseErrorCode {
            fn from(e: &usvg::Error) -> Self {
                match e {
                    usvg::Error::NotAnUtf8Str => Self::NotAnUtf8Str,
                    usvg::Error::SvgzFeatureNotEnabled => Self::SvgzFeatureNotEnabled,
                    usvg::Error::MalformedGZip => Self::MalformedGZip,
                    usvg::Error::ElementsLimitReached => Self::ElementsLimitReached,
                    usvg::Error::InvalidSize => Self::InvalidSize,
                    usvg::Error::ParsingFailed(_) => Self::ParsingFailed,
                }
            }
        }

        #[doc = " The JS error for a parse failure, `code` included. napi's own"]
        #[doc = " `Error` can only carry its fixed `Status` as the code, and one built"]
        #[doc = " back from a JS object loses the object on WASI, so both paths below"]
        #[doc = " hand this object to JS themselves."]
        fn parse_error<'e>(env: &'e Env, e: &usvg::Error) -> Result<Object<'e>> {
            // usvg's text names a cargo feature: wrong advice for JS.
            let reason = match e {
                usvg::Error::SvgzFeatureNotEnabled => {
                    "SVGZ is not supported: inflate it first, e.g. zlib.gunzipSync(data, { maxOutputLength })".to_string()
                }
                e => e.to_string(),
            };
            let mut error = env.create_error(Error::from_reason(format!("invalid SVG: {reason}")))?;
            error.set_named_property("code", ParseErrorCode::from(e))?;
            Ok(error)
        }

        #[doc = " Thrown from a sync call: the error is thrown here, and the one"]
        #[doc = " returned only says it is pending, so napi does not throw again."]
        fn throw_parse(env: &Env, e: &usvg::Error) -> Error {
            match parse_error(env, e).and_then(|error| env.throw(error)) {
                Ok(()) => Error::new(Status::PendingException, format!("invalid SVG: {e}")),
                Err(err) => err,
            }
        }

        #[doc = " From a worker-thread task: a rejected promise, which the task's"]
        #[doc = " own promise adopts on resolve."]
        fn rejected_parse<'e>(env: &'e Env, e: &usvg::Error) -> Result<Unknown<'e>> {
            PromiseRaw::<Unknown>::reject(env, parse_error(env, e)?).map(|p| p.to_unknown())
        }

        #[doc = " One command of a path outline, in the document's own units."]
        #[doc = ""]
        #[doc = " `points` holds x,y pairs, and how many depends on `type`: one point"]
        #[doc = " for `moveTo` and `lineTo`, two for `quadTo` (control, end), three"]
        #[doc = " for `cubicTo` (two controls, end), none for `close`."]
        #[doc = ""]
        #[doc = " A flat list rather than a variant per command: a path can carry"]
        #[doc = " thousands of segments, and one tagged object each is already the"]
        #[doc = " expensive part."]
        #[napi(object)]
        #[derive(Clone)]
        pub struct PathSegment {
            #[napi(ts_type = "'moveTo' | 'lineTo' | 'quadTo' | 'cubicTo' | 'close'")]
            pub r#type: String,
            pub points: Vec<f64>,
        }

        #[doc = " tiny-skia stores a path as a verb stream, not a struct, so there is"]
        #[doc = " nothing for the mapper to walk -- hence this by hand."]
        fn path_segments(p: &tiny_skia::Path) -> Vec<PathSegment> {
            let seg = |kind: &str, pts: &[tiny_skia::Point]| PathSegment {
                r#type: kind.to_string(),
                points: pts.iter().flat_map(|p| [f64::from(p.x), f64::from(p.y)]).collect(),
            };
            p.segments()
                .map(|s| match s {
                    tiny_skia::PathSegment::MoveTo(a) => seg("moveTo", &[a]),
                    tiny_skia::PathSegment::LineTo(a) => seg("lineTo", &[a]),
                    tiny_skia::PathSegment::QuadTo(a, b) => seg("quadTo", &[a, b]),
                    tiny_skia::PathSegment::CubicTo(a, b, c) => seg("cubicTo", &[a, b, c]),
                    tiny_skia::PathSegment::Close => seg("close", &[]),
                })
                .collect()
        }

        #[doc = " A parsed SVG, ready to be rendered any number of times."]
        #[napi]
        #[doc = ""]
        // `Clone` because an async twin captures the receiver: every field is
        // behind an `Arc`, the pending lists included, so the clone is a few
        // refcount bumps and the worker thread never touches the JS heap.
        // The image map used to be copied whole: 40 us per call with 500 images.
        #[derive(Clone)]
        pub struct Resvg {
            tree: std::sync::Arc<usvg::Tree>,
            #[doc = " Source kept verbatim: resolving an image means re-parsing."]
            svg: std::sync::Arc<Vec<u8>>,
            options: std::sync::Arc<RenderOptions>,
            fonts: std::sync::Arc<usvg::fontdb::Database>,
            #[doc = " Copied on write, by `resolveImage` only."]
            images: std::sync::Arc<ImageMap>,
            pending_images: std::sync::Arc<[String]>,
            pending_fonts: std::sync::Arc<[String]>,
            logs: Logs,
            #[doc = " Set by `dispose`: every method in `impl Resvg` checks it (emit.rs)."]
            disposed: bool,
        }

        #[napi]
        impl Resvg {
            #[napi(constructor)]
            pub fn new(
                env: Env,
                svg: Either<String, Uint8Array>,
                options: Option<RenderOptions>,
                fonts: Option<&FontDatabase>,
                images: Option<std::collections::HashMap<String, Uint8Array>>,
            ) -> Result<Self> {
                let svg = svg_bytes(svg);
                let options = std::sync::Arc::new(options.unwrap_or_default());
                let fonts = match fonts {
                    Some(f) => f.inner.0.clone(),
                    None => default_fontdb(),
                };
                let images = std::sync::Arc::new(image_map(images));
                let logs = Logs::default();
                let (tree, pending_images, pending_fonts) =
                    logs.scope(|| Self::build(&svg, &options, fonts.clone(), &images))?
                        .map_err(|e| throw_parse(&env, &e))?;
                Ok(Self {
                    tree: std::sync::Arc::new(tree),
                    svg: std::sync::Arc::new(svg),
                    options,
                    fonts,
                    images,
                    pending_images,
                    pending_fonts,
                    logs,
                    disposed: false,
                })
            }

            #[doc = " Frees the parsed tree, the source and the images now, not at garbage"]
            #[doc = " collection; every later call throws. Also `[Symbol.dispose]` where"]
            #[doc = " the runtime has it, so `using doc = new Resvg(svg)` frees it at the"]
            #[doc = " end of the block. An async call holding the tree keeps it."]
            #[napi]
            pub fn dispose(&mut self) {
                static EMPTY: std::sync::OnceLock<std::sync::Arc<usvg::Tree>> = std::sync::OnceLock::new();
                self.tree = EMPTY
                    .get_or_init(|| {
                        let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>";
                        std::sync::Arc::new(usvg::Tree::from_str(svg, &usvg::Options::default()).expect("a constant SVG parses"))
                    })
                    .clone();
                self.svg = Default::default();
                self.images = Default::default();
                self.pending_images = Vec::new().into();
                self.pending_fonts = Vec::new().into();
                self.disposed = true;
            }

            #[doc = " Whether `dispose` has been called."]
            #[napi(getter)]
            pub fn disposed(&self) -> bool {
                self.disposed
            }

            #[doc = " Hrefs of `<image>` elements that neither the supplied buffers nor the"]
            #[doc = " filesystem could resolve. Feed them back with `resolveImage`."]
            #[napi]
            pub fn pending_images(&self) -> Vec<String> {
                self.pending_images.to_vec()
            }

            #[doc = " Drains what usvg and resvg reported while parsing and rendering"]
            #[doc = " this document, async twins included, and nothing from any other."]
            #[doc = " Collected only once `setLogLevel` has set a level."]
            #[napi(js_name = "takeLogs")]
            pub fn take_own_logs(&self) -> Vec<String> {
                self.logs.take()
            }

            #[doc = " Named font families requested by `<text>` that the database does not"]
            #[doc = " carry, so the text was rendered with a fallback face. Load them with"]
            #[doc = " `FontDatabase` and re-parse to fix the output."]
            #[napi]
            pub fn pending_fonts(&self) -> Vec<String> {
                self.pending_fonts.to_vec()
            }

            #[doc = " Supplies one image and re-parses the document."]
            #[doc = ""]
            #[doc = " Pass everything you already have through the constructor's `images`"]
            #[doc = " argument, or `resolveImages`, instead: this re-parses once per call."]
            #[napi]
            pub fn resolve_image(&mut self, href: String, data: Uint8Array) -> Result<()> {
                std::sync::Arc::make_mut(&mut self.images)
                    .insert(href, std::sync::Arc::new(data.to_vec()));
                self.reparse()
            }

            #[doc = " Supplies several images and re-parses the document once: what"]
            #[doc = " `pendingImages()` asked for, fetched, handed back in one call."]
            #[doc = " An href the document does not use is kept, not an error."]
            #[napi]
            pub fn resolve_images(&mut self, images: std::collections::HashMap<String, Uint8Array>) -> Result<()> {
                std::sync::Arc::make_mut(&mut self.images).extend(image_map(Some(images)));
                self.reparse()
            }

            fn reparse(&mut self) -> Result<()> {
                let (tree, pending_images, pending_fonts) = self.logs.scope(|| {
                    Self::build(&self.svg, &self.options, self.fonts.clone(), &self.images)
                })?
                // Unreachable in practice: the constructor already parsed this
                // source with these options, and an image never fails the parse.
                .map_err(|e| Error::from_reason(format!("invalid SVG: {e}")))?;
                self.tree = std::sync::Arc::new(tree);
                self.pending_images = pending_images;
                self.pending_fonts = pending_fonts;
                Ok(())
            }

            #[napi(getter)]
            pub fn width(&self) -> f64 {
                self.tree.size().width().into()
            }

            #[napi(getter)]
            pub fn height(&self) -> f64 {
                self.tree.size().height().into()
            }

            #[doc = " Rasterise and encode: the Send half, so the derived twin can run it"]
            #[doc = " on a worker thread."]
            #[async_twin(render_png_async, Buffer)]
            fn png_bytes(&self, params: Option<RenderParams>) -> Result<Vec<u8>> {
                encode_png(&self.draw(params.unwrap_or_default())?)
            }

            #[doc = " Renders to a PNG buffer."]
            #[napi]
            pub fn render_png(&self, params: Option<RenderParams>) -> Result<Buffer> {
                self.png_bytes(params).map(IntoJs::into_js)
            }

            #[doc = " Parses on a worker thread. Same arguments as the constructor, and the"]
            #[doc = " resolved instance still reports `pendingImages` / `pendingFonts`."]
            #[napi(ts_return_type = "Promise<Resvg>")]
            pub fn parse_async(
                svg: Either<String, Uint8Array>,
                options: Option<RenderOptions>,
                fonts: Option<&FontDatabase>,
                images: Option<std::collections::HashMap<String, Uint8Array>>,
                #[napi(ts_arg_type = "AbortSignal | undefined | null")] signal: Option<Signal>,
            ) -> AsyncTask<ParseTask> {
                AsyncTask::with_optional_signal(
                    ParseTask {
                        svg: svg_bytes(svg),
                        options: std::sync::Arc::new(options.unwrap_or_default()),
                        // The system database is left to `compute`: the first
                        // enumeration scans every installed font, on whichever
                        // thread asks first, and this one is the event loop.
                        fonts: fonts.map(|f| f.inner.0.clone()),
                        images: std::sync::Arc::new(image_map(images)),
                        logs: Logs::default(),
                    },
                    signal.map(|s| s.0),
                )
            }

            #tree_methods

            #group_methods

            #[doc = " Writes the parsed tree back out as a usvg-simplified SVG string."]
            #[doc = ""]
            #[doc = " Text is converted to paths unless `preserveText` is set."]
            #[napi(js_name = "toString")]
            pub fn to_svg_string(&self, options: Option<WriteOptions>) -> Result<String> {
                self.svg_text(options)
            }

            #[doc = " Serialising a large tree is not free; this is the half a twin runs."]
            #[async_twin(to_string_async, String)]
            fn svg_text(&self, options: Option<WriteOptions>) -> Result<String> {
                let opt = options.unwrap_or_default().to_usvg()?;
                let tree = &self.tree;
                // Numbers come from JS and the writer indexes a few tables
                // unchecked; an unwind reaching the extern "C" frame would abort
                // the process, so stop it here.
                self.logs.scope(|| contained("usvg's SVG writer", || usvg::Tree::to_string(tree, &opt)))
            }

            #[doc = " Rasterise only: width, height and the demultiplied pixels, all Send."]
            #[async_twin(render_raw_async, RawImage)]
            fn raw_pixels(&self, params: Option<RenderParams>) -> Result<(u32, u32, Vec<u8>)> {
                let pixmap = self.draw(params.unwrap_or_default())?;
                let (width, height) = (pixmap.width(), pixmap.height());
                Ok((width, height, pixmap.take_demultiplied()))
            }

            #[doc = " Renders to raw RGBA8 pixels."]
            #[napi]
            pub fn render_raw(&self, params: Option<RenderParams>) -> Result<RawImage> {
                self.raw_pixels(params).map(IntoJs::into_js)
            }
        }

        impl Resvg {
            #[doc = " The check emit.rs puts first in every `Resvg` method."]
            fn live(&self) -> Result<()> {
                if self.disposed {
                    return Err(Error::from_reason("this Resvg was disposed"));
                }
                Ok(())
            }
        }

        impl Resvg {
            fn build(
                svg: &[u8],
                options: &RenderOptions,
                fonts: std::sync::Arc<usvg::fontdb::Database>,
                images: &std::sync::Arc<ImageMap>,
            ) -> Result<std::result::Result<Parsed, usvg::Error>> {
                let missing_images = Misses::default();
                let missing_fonts = Misses::default();
                let mut opts = options.to_usvg(fonts);
                opts.image_href_resolver =
                    href_resolver(
                        images.clone(),
                        missing_images.clone(),
                        options.resources_root.as_ref().map(std::path::PathBuf::from),
                    );
                opts.font_resolver = font_resolver(missing_fonts.clone());
                let tree = match contained("usvg", || usvg::Tree::from_data(svg, &opts))? {
                    Ok(tree) => tree,
                    Err(e) => return Ok(Err(e)),
                };
                Ok(Ok((
                    tree,
                    missing_images.take(),
                    missing_fonts.take(),
                )))
            }

            fn draw(&self, p: RenderParams) -> Result<tiny_skia::Pixmap> {
                self.logs.scope(|| draw(&self.tree, &p))
            }
        }

        #[doc = " `Resvg.prototype[Symbol.dispose]`, only where the runtime has the"]
        #[doc = " symbol: napi's `well_known_symbol` fails the whole module without it"]
        #[doc = " (iOS Safari, as of 2026). The type is declared by `napi.dtsHeader`."]
        #[napi(module_exports)]
        pub fn alias_dispose(env: Env, exports: Object) -> Result<()> {
            let symbol: Object = env.get_global()?.get_named_property_unchecked("Symbol")?;
            let key: Unknown = symbol.get_named_property("dispose")?;
            if key.get_type()? != ValueType::Symbol {
                return Ok(());
            }
            let class: Object = exports.get_named_property_unchecked("Resvg")?;
            let mut proto: Object = class.get_named_property_unchecked("prototype")?;
            let dispose: Unknown = proto.get_named_property("dispose")?;
            proto.set_property(key, dispose)
        }

        #[doc = " PNG-encodes a rendered pixmap, inside `contained` like the render."]
        fn encode_png(pixmap: &tiny_skia::Pixmap) -> Result<Vec<u8>> {
            contained("PNG encoder", || pixmap.encode_png())?
                .map_err(|e| Error::from_reason(format!("PNG encoding failed: {e}")))
        }

        fn draw(tree: &usvg::Tree, p: &RenderParams) -> Result<tiny_skia::Pixmap> {
                let size = tree.size();
                // A crop replaces the viewport: width/height/scale then size the
                // cropped region, which is what you want after a bbox call.
                let (base_w, base_h, off_x, off_y) = match &p.crop {
                    Some(c) => (to_f32(c.width), to_f32(c.height), to_f32(c.x), to_f32(c.y)),
                    None => (size.width(), size.height(), 0.0, 0.0),
                };
                if !(base_w > 0.0 && base_h > 0.0) {
                    return Err(Error::from_reason(format!(
                        "empty crop: {base_w}x{base_h}"
                    )));
                }
                // A NaN offset rendered a blank image without a word.
                if !(off_x.is_finite() && off_y.is_finite() && base_w.is_finite() && base_h.is_finite()) {
                    return Err(Error::from_reason(format!(
                        "invalid crop: {off_x},{off_y} {base_w}x{base_h}"
                    )));
                }
                let (scale, mut pixmap) = canvas(base_w, base_h, p)?;
                contained("resvg", || resvg::render(
                    tree,
                    tiny_skia::Transform::from_scale(scale, scale)
                        .pre_translate(-off_x, -off_y),
                    &mut pixmap.as_mut(),
                ))?;
                Ok(pixmap)
        }

        // ponytail: the only `as` left in the runtime. Narrowing a float has no
        // std conversion; `as` rounds to nearest and saturates, which is the
        // behaviour wanted here (maxPixels rejects a saturated size after).
        fn to_f32(v: f64) -> f32 {
            v as f32
        }
        fn to_px(v: f64) -> u32 {
            (v as u32).max(1)
        }
        fn ratio(px: u32, base: f32) -> f32 {
            to_f32(f64::from(px)) / base
        }

        #[doc = " Size, scale and background of one render pass."]
        fn canvas(base_w: f32, base_h: f32, p: &RenderParams) -> Result<(f32, tiny_skia::Pixmap)> {
                // A requested dimension is honoured exactly, not recomputed.
                // `(base * (w / base)).ceil()` looks like an identity and is not:
                // the f32 round trip can land a hair above the integer and the
                // ceil then adds a pixel. On a 100x50 document seventeen of the
                // first four hundred widths came back one too wide -- 120 gave a
                // PNG whose IHDR read 121x61 -- while every width the test suite
                // uses is an exact multiple, so nothing here could see it.
                // f64 rather than napi's u32, which wraps: -1 became 4294967295
                // and 2**32 + 10 rendered 10 px wide.
                let px = |v: Option<f64>, name: &str| -> Result<Option<u32>> {
                    match v.map(f64::round) {
                        None => Ok(None),
                        Some(v) if (1.0..=f64::from(u32::MAX)).contains(&v) => Ok(Some(to_px(v))),
                        Some(_) => Err(Error::from_reason(format!(
                            "invalid {name}: {}", v.unwrap_or_default()
                        ))),
                    }
                };
                let (width, height) = (px(p.width, "width")?, px(p.height, "height")?);
                let (scale, w, h) = if let Some(w) = width {
                    // The other side from the ratio itself, in f64: going through
                    // the f32 scale makes 50 * (120 / 100) come out 60.000004,
                    // which ceils to 61.
                    let h = (f64::from(base_h) * f64::from(w) / f64::from(base_w)).ceil();
                    (ratio(w, base_w), w, to_px(h))
                } else if let Some(h) = height {
                    let w = (f64::from(base_w) * f64::from(h) / f64::from(base_h)).ceil();
                    (ratio(h, base_h), to_px(w), h)
                } else {
                    let s = p.scale.unwrap_or(1.0);
                    (
                        to_f32(s),
                        to_px((f64::from(base_w) * s).ceil()),
                        to_px((f64::from(base_h) * s).ceil()),
                    )
                };
                if !(scale.is_finite() && scale > 0.0) {
                    return Err(Error::from_reason(format!("invalid scale: {scale}")));
                }
                let max = p.max_pixels.unwrap_or(67_108_864.0); // 2^26
                if !(max.is_finite() && max > 0.0) {
                    return Err(Error::from_reason(format!("invalid maxPixels: {max}")));
                }
                if f64::from(w) * f64::from(h) > max {
                    return Err(Error::from_reason(format!(
                        "{w}x{h} exceeds maxPixels ({max})"
                    )));
                }
                let mut pixmap = tiny_skia::Pixmap::new(w, h)
                    .ok_or_else(|| Error::from_reason(format!("bad pixmap size {w}x{h}")))?;
                if let Some(css) = &p.background {
                    // svgtypes is resvg's own colour parser, so `rgba()`, `hsl()`
                    // and the CSS colour names behave exactly as in an SVG attribute.
                    let c: svgtypes::Color = css
                        .parse()
                        .map_err(|_| Error::from_reason(format!("invalid background: {css}")))?;
                    pixmap.fill(tiny_skia::Color::from_rgba8(c.red, c.green, c.blue, c.alpha));
                }
                Ok((scale, pixmap))
        }

        #[doc = " Parse on a libuv worker thread, resolving to a ready `Resvg`."]
        pub struct ParseTask {
            svg: Vec<u8>,
            options: std::sync::Arc<RenderOptions>,
            #[doc = " `None` for the system fonts, resolved in `compute`."]
            fonts: Option<std::sync::Arc<usvg::fontdb::Database>>,
            images: std::sync::Arc<ImageMap>,
            logs: Logs,
        }

        impl<'env> napi::ScopedTask<'env> for ParseTask {
            type Output = std::result::Result<Parsed, usvg::Error>;
            type JsValue = Either<Resvg, Unknown<'env>>;

            fn compute(&mut self) -> Result<Self::Output> {
                let fonts = self.fonts.get_or_insert_with(default_fontdb).clone();
                self.logs
                    .scope(|| Resvg::build(&self.svg, &self.options, fonts, &self.images))
            }

            fn resolve(&mut self, env: &'env Env, out: Self::Output) -> Result<Self::JsValue> {
                let (tree, pending_images, pending_fonts) = match out {
                    Ok(parsed) => parsed,
                    Err(e) => return rejected_parse(env, &e).map(Either::B),
                };
                Ok(Either::A(Resvg {
                    tree: std::sync::Arc::new(tree),
                    svg: std::sync::Arc::new(std::mem::take(&mut self.svg)),
                    options: std::mem::take(&mut self.options),
                    fonts: self.fonts.take().unwrap_or_else(default_fontdb),
                    images: std::mem::take(&mut self.images),
                    pending_images,
                    pending_fonts,
                    logs: std::mem::take(&mut self.logs),
                    disposed: false,
                }))
            }
        }

        #[doc = " Parse, rasterise and PNG-encode in one worker-thread round trip."]
        pub struct RenderTask {
            svg: Vec<u8>,
            options: RenderOptions,
            params: RenderParams,
        }

        impl<'env> napi::ScopedTask<'env> for RenderTask {
            type Output = std::result::Result<Vec<u8>, usvg::Error>;
            type JsValue = Either<Buffer, Unknown<'env>>;

            fn compute(&mut self) -> Result<Self::Output> {
                let tree = match Resvg::build(
                    &self.svg,
                    &self.options,
                    default_fontdb(),
                    &Default::default(),
                )? {
                    Ok((tree, _, _)) => tree,
                    Err(e) => return Ok(Err(e)),
                };
                encode_png(&draw(&tree, &self.params)?).map(Ok)
            }

            fn resolve(&mut self, env: &'env Env, png: Self::Output) -> Result<Self::JsValue> {
                match png {
                    Ok(png) => Ok(Either::A(Buffer::from(png))),
                    Err(e) => rejected_parse(env, &e).map(Either::B),
                }
            }
        }

        #[doc = " One-shot: parse and render a PNG entirely off the event loop."]
        #[doc = ""]
        #[doc = " Uses the process-wide system font database and resolves images off the"]
        #[doc = " filesystem only. For a custom `FontDatabase`, supplied image buffers or"]
        #[doc = " the `pendingImages` / `pendingFonts` reports, use `Resvg.parseAsync`."]
        #[napi(ts_return_type = "Promise<Buffer>")]
        pub fn render_async(
            svg: Either<String, Uint8Array>,
            options: Option<RenderOptions>,
            params: Option<RenderParams>,
            #[napi(ts_arg_type = "AbortSignal | undefined | null")] signal: Option<Signal>,
        ) -> AsyncTask<RenderTask> {
            AsyncTask::with_optional_signal(
                RenderTask {
                    svg: svg_bytes(svg),
                    options: options.unwrap_or_default(),
                    params: params.unwrap_or_default(),
                },
                signal.map(|s| s.0),
            )
        }
    }
}
