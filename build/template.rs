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
/// signature: these are the same on both of its calls, while the four method
/// lists are not.
pub struct Fragments<'a> {
    pub enums: TokenStream,
    pub object_code: TokenStream,
    pub wrapper_code: TokenStream,
    pub decls: Vec<&'a TokenStream>,
    pub assigns: Vec<&'a TokenStream>,
    pub write_decls: Vec<&'a TokenStream>,
    pub write_assigns: Vec<&'a TokenStream>,
}

impl Fragments<'_> {
    /// Every fragment empty, for probing the template's own body. What the
    /// pruner needs from it -- which generated types it names -- is written in
    /// the template itself, never in what gets interpolated, so the fragments
    /// can be blank. Same double-build trick `template_fns` uses to read the
    /// method names off it.
    pub fn probe() -> Fragments<'static> {
        Fragments {
            enums: TokenStream::new(),
            object_code: TokenStream::new(),
            wrapper_code: TokenStream::new(),
            decls: Vec::new(),
            assigns: Vec::new(),
            write_decls: Vec::new(),
            write_assigns: Vec::new(),
        }
    }
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
    node_methods: &TokenStream,
) -> TokenStream {
    // Destructured by name so the body below is unchanged from when it was a
    // closure capturing these from `main`.
    let Fragments {
        enums,
        object_code,
        wrapper_code,
        decls,
        assigns,
        write_decls,
        write_assigns,
    } = f;
    quote! {
        use napi::bindgen_prelude::*;
        use napi_derive::napi;
        use resvg::{tiny_skia, usvg};

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

        #[doc = " The bytes of an SVG handed over as text or as a Buffer."]
        fn svg_bytes(svg: Either<String, Buffer>) -> Vec<u8> {
            match svg {
                Either::A(text) => text.into_bytes(),
                Either::B(data) => data.to_vec(),
            }
        }

        #[doc = " JS `{ href: Buffer }` -> the map the href resolver reads, copied"]
        #[doc = " off the JS heap so a worker thread can hold it."]
        fn image_map(images: Option<std::collections::HashMap<String, Buffer>>) -> ImageMap {
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

        #[doc = " `usvg::ImageHrefResolver` demands `Send + Sync`, so a JS callback cannot"]
        #[doc = " live in it. Buffers are handed over up front instead, and whatever stays"]
        #[doc = " unresolved is reported back to JS for a second pass."]
        fn href_resolver(
            images: std::sync::Arc<ImageMap>,
            misses: Misses,
            root: Option<std::path::PathBuf>,
        ) -> usvg::ImageHrefResolver<'static> {
            let from_disk = usvg::ImageHrefResolver::default_string_resolver();
            let sniff = usvg::ImageHrefResolver::default_data_resolver();
            usvg::ImageHrefResolver {
                resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
                resolve_string: Box::new(move |href: &str, opts: &usvg::Options| {
                    // 1. buffer supplied by JS -- let usvg sniff the format itself
                    if let Some(data) = images.get(href) {
                        return sniff("text/plain", data.clone(), opts);
                    }
                    // 2. a file under `resourcesDir`, and nowhere else. usvg's
                    // default reads any absolute path, and a relative one off
                    // the working directory, so an untrusted document could
                    // pull local files into its pixels or `toString()` output,
                    // or exhaust memory reading `/dev/zero`.
                    if let Some(path) = under_resources_dir(href, opts, root.as_deref()) {
                        if let Some(kind) = from_disk(&path.to_string_lossy(), opts) {
                            return Some(kind);
                        }
                    }
                    // 3. give up, but tell JS about it
                    // Poison-tolerant: this mutex only accumulates
                    // diagnostics, so a panic elsewhere must not take the
                    // render down with it.
                    misses.0.lock().unwrap_or_else(|e| e.into_inner()).push(href.to_string());
                    None
                }),
            }
        }

        #[doc = " `href` resolved against `resourcesDir`, if it names a regular file"]
        #[doc = " inside `root` (default: `resourcesDir` itself). Canonicalised, so"]
        #[doc = " `..` and symlinks cannot step outside it."]
        fn under_resources_dir(
            href: &str,
            opts: &usvg::Options,
            root: Option<&std::path::Path>,
        ) -> Option<std::path::PathBuf> {
            let dir = opts.resources_dir.as_deref()?;
            let root = root.unwrap_or(dir).canonicalize().ok()?;
            let path = dir.join(href).canonicalize().ok()?;
            (path.starts_with(&root) && path.is_file()).then_some(path)
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
            #[doc = " Takes the `Doc` every generated handle is built with, and drops"]
            #[doc = " it: the database is what a document points at, not part of one."]
            fn wrap(inner: std::sync::Arc<usvg::fontdb::Database>, _doc: Doc) -> Self {
                Self { inner: SharedDb(inner) }
            }

            #[doc = " Empty on purpose: a face handed out with this database's `Arc`"]
            #[doc = " would hold a second reference, and the next `loadFont*` would"]
            #[doc = " then deep-copy the whole database on write."]
            fn doc(&self) -> Doc {
                Doc::default()
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
                self.inner.face(id).cloned().map(|face| FontFace::wrap(face, self.doc()))
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
                Some(Either::A(n)) => u8::try_from(*n)
                    .map(usvg::Indent::Spaces)
                    .map_err(|_| Error::from_reason(format!("invalid {name}: {n}"))),
                Some(Either::B(s)) => match s.as_str() {
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
            #[doc = " Refuse to allocate more than this many pixels (4 bytes each)."]
            #[doc = " Default: 2^28, a 16384x16384 canvas or 1 GiB. Without a cap a"]
            #[doc = " document declaring `width=\"200000\"` aborts the whole process"]
            #[doc = " on the failed allocation, which no try/catch can recover."]
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
                                let mut seen = misses.0.lock().unwrap_or_else(|e| e.into_inner());
                                if !seen.iter().any(|s| s == name) {
                                    seen.push(name.clone());
                                }
                            }
                        }
                        select(font, db)
                    },
                ),
                select_fallback: usvg::FontResolver::default_fallback_selector(),
            }
        }

        #object_code

        #wrapper_code

        #[doc = " Which flavour of node this is."]
        #[napi(string_enum = "camelCase")]
        #[derive(Copy, Clone)]
        pub enum NodeKind {
            Group,
            Path,
            Image,
            Text,
        }

        #[doc = " Walks a child-index path down from the root group."]
        #[doc = " usvg and resvg report every recoverable problem through the `log` crate"]
        #[doc = " -- unsupported elements, unparsable values, skipped images. Nothing"]
        #[doc = " consumes it by default, so those messages are lost; this buffers them."]
        static LOGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

        const LOG_CAP: usize = 500;

        thread_local! {
            #[doc = " Set while a document parses or renders on this thread, so its"]
            #[doc = " messages can be told apart from a concurrent render's."]
            static LOG_SINK: std::cell::RefCell<Option<Vec<String>>> =
                const { std::cell::RefCell::new(None) };
        }

        #[doc = " One document's messages, shared with the clones its async twins run on."]
        #[derive(Default, Clone)]
        struct Logs(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

        impl Logs {
            #[doc = " Runs `f` with this thread's messages routed here as well."]
            fn scope<T>(&self, f: impl FnOnce() -> T) -> T {
                let outer = LOG_SINK.with(|s| s.replace(Some(Vec::new())));
                let out = f();
                let got = LOG_SINK.with(|s| s.replace(outer)).unwrap_or_default();
                let mut buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
                let room = LOG_CAP.saturating_sub(buf.len());
                buf.extend(got.into_iter().take(room));
                out
            }
        }

        struct LogCollector;
        static LOG_COLLECTOR: LogCollector = LogCollector;

        impl log::Log for LogCollector {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }

            fn log(&self, record: &log::Record) {
                let line = format!("{} {}: {}", record.level(), record.target(), record.args());
                LOG_SINK.with(|s| {
                    if let Some(buf) = s.borrow_mut().as_mut() {
                        if buf.len() < LOG_CAP {
                            buf.push(line.clone());
                        }
                    }
                });
                if let Ok(mut buf) = LOGS.lock() {
                    // Bounded: a pathological document must not grow this forever.
                    if buf.len() < LOG_CAP {
                        buf.push(line);
                    }
                }
            }

            fn flush(&self) {}
        }

        #[doc = " Starts collecting what usvg and resvg report."]
        #[doc = ""]
        #[doc = " `level` is `off`, `error`, `warn`, `info`, `debug` or `trace`. Safe to"]
        #[doc = " call repeatedly: the logger is installed once, the level always applies."]
        // The six levels are checked at runtime and were declared as a bare
        // `string`, so a typo was a thrown error rather than a red squiggle.
        #[napi(ts_args_type = "level: 'off' | 'error' | 'warn' | 'info' | 'debug' | 'trace'")]
        pub fn set_log_level(level: String) -> Result<()> {
            let filter: log::LevelFilter = level
                .parse()
                .map_err(|_| Error::from_reason(format!("unknown log level: {level:?}")))?;
            static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            INSTALLED.get_or_init(|| {
                let _ = log::set_logger(&LOG_COLLECTOR);
            });
            log::set_max_level(filter);
            Ok(())
        }

        #[doc = " Drains the messages collected since the last call, from every"]
        #[doc = " document and thread. Concurrent renders interleave here; a"]
        #[doc = " document's own `takeLogs()` holds only its messages."]
        #[napi]
        pub fn take_logs() -> Vec<String> {
            LOGS.lock()
                .map(|mut buf| std::mem::take(&mut *buf))
                .unwrap_or_default()
        }

        #[doc = " Anything that owns a content group: the document, or a def such as a"]
        #[doc = " pattern, a mask or a clip path. The generator implements it for every"]
        #[doc = " wrapped type that has a `root() -> &Group`."]
        #[doc = " Send + Sync: a node reached through a def keeps its owner alive, and"]
        #[doc = " an async twin carries that owner to a worker thread."]
        pub trait HasRoot: Send + Sync {
            fn group(&self) -> &usvg::Group;
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

        #[doc = " `Font::families` is `&[FontFamily]`, an enum svgtypes owns: five CSS"]
        #[doc = " keywords plus `Named(String)`. No rule maps an enum from a third"]
        #[doc = " crate, and without it `faceOf` answers half a question -- which face"]
        #[doc = " drew the text, with nothing to compare it against."]
        #[doc = " Decision: the strings usvg's own writer emits, keywords included."]
        #[napi]
        impl Font {
            #[doc = " Families this span asked for, in order. Never empty: usvg appends"]
            #[doc = " `RenderOptions.fontFamily` as the last resort."]
            #[napi(getter)]
            pub fn families(&self) -> Vec<String> {
                self.inner
                    .families()
                    .iter()
                    .map(|family| match family {
                        usvg::FontFamily::Named(name) => name.clone(),
                        usvg::FontFamily::Serif => "serif".to_string(),
                        usvg::FontFamily::SansSerif => "sans-serif".to_string(),
                        usvg::FontFamily::Cursive => "cursive".to_string(),
                        usvg::FontFamily::Fantasy => "fantasy".to_string(),
                        usvg::FontFamily::Monospace => "monospace".to_string(),
                    })
                    .collect()
            }
        }

        #[doc = " The document a class was reached from: what a node, a def or a"]
        #[doc = " glyph needs to resolve the references it carries -- the def tables"]
        #[doc = " (`tree`), the font database its IDs belong to (`fonts`), and the"]
        #[doc = " log its renders report to. Empty for what no document handed out."]
        #[doc = " Three refcounts, cloned into every class the generator emits."]
        #[derive(Clone, Default)]
        struct Doc {
            tree: Option<std::sync::Arc<usvg::Tree>>,
            fonts: Option<std::sync::Arc<usvg::fontdb::Database>>,
            logs: Logs,
        }

        #[doc = " The shared definition `target` refers to, by identity. An `id`"]
        #[doc = " match missed defs without one and could pick a namesake."]
        fn find_def<T>(defs: &[std::sync::Arc<T>], target: &T) -> Option<std::sync::Arc<T>> {
            defs.iter()
                .find(|d| std::ptr::eq(std::sync::Arc::as_ptr(d), target))
                .cloned()
        }

        impl SvgNode {
            fn doc(&self) -> Doc {
                self.doc.clone()
            }
        }

        #[napi]
        impl ClipPath {
            #[doc = " The clip path applied to this clip path's own content, if any."]
            #[doc = " `null` too when the handle came from no document."]
            #[napi]
            pub fn clip_path(&self) -> Option<ClipPath> {
                let target = self.inner.clip_path()?;
                let tree = self.doc.tree.as_ref()?;
                find_def(tree.clip_paths(), target).map(|c| ClipPath::wrap(c, self.doc()))
            }
        }

        #[napi]
        impl Mask {
            #[doc = " The mask applied to this mask's own content, if any. `null` too"]
            #[doc = " when the handle came from no document."]
            #[napi]
            pub fn mask(&self) -> Option<Mask> {
                let target = self.inner.mask()?;
                let tree = self.doc.tree.as_ref()?;
                find_def(tree.masks(), target).map(|m| Mask::wrap(m, self.doc()))
            }
        }

        #[napi]
        impl PositionedGlyph {
            #[doc = " The face that drew this glyph, from the database its document"]
            #[doc = " was parsed with. `null` for a glyph that came from no document."]
            #[napi(getter)]
            pub fn font(&self) -> Option<FontFace> {
                let fonts = self.doc.fonts.as_ref()?;
                fonts
                    .face(self.inner.font)
                    .map(|face| FontFace::wrap(face.clone(), self.doc()))
            }
        }

        #[doc = " Where a node path starts from."]
        #[derive(Clone)]
        enum NodeBase {
            Tree(std::sync::Arc<usvg::Tree>),
            Def(std::sync::Arc<dyn HasRoot>),
        }

        impl NodeBase {
            fn group(&self) -> &usvg::Group {
                match self {
                    NodeBase::Tree(t) => t.root(),
                    NodeBase::Def(d) => d.group(),
                }
            }
        }

        fn node_at<'t>(root: &'t usvg::Group, path: &[usize]) -> Option<&'t usvg::Node> {
            let (last, rest) = path.split_last()?;
            let mut group = root;
            for i in rest {
                match group.children().get(*i)? {
                    usvg::Node::Group(g) => group = g,
                    _ => return None,
                }
            }
            group.children().get(*last)
        }

        #[doc = " Depth-first search for an element id, returning its index path."]
        fn path_of_id(group: &usvg::Group, id: &str, prefix: &mut Vec<usize>) -> Option<Vec<usize>> {
            for (i, child) in group.children().iter().enumerate() {
                prefix.push(i);
                if child.id() == id {
                    return Some(prefix.clone());
                }
                if let usvg::Node::Group(g) = child {
                    if let Some(found) = path_of_id(g, id, prefix) {
                        return Some(found);
                    }
                }
                prefix.pop();
            }
            None
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

        #[doc = " A read-only handle on one element of the parsed tree."]
        #[doc = ""]
        #[doc = " usvg stores nodes as `Box`, not `Arc`, so a handle cannot own one. It"]
        #[doc = " keeps the tree alive plus the child-index path instead, and re-resolves"]
        #[doc = " on each call -- safe because a parsed tree is immutable."]
        #[napi]
        #[derive(Clone)]
        pub struct SvgNode {
            doc: Doc,
            base: NodeBase,
            path: Vec<usize>,
        }

        impl SvgNode {
            fn node(&self) -> Result<&usvg::Node> {
                node_at(self.base.group(), &self.path)
                    .ok_or_else(|| Error::from_reason("node path no longer resolves"))
            }

            fn child(&self, i: usize) -> Self {
                let mut path = self.path.clone();
                path.push(i);
                Self { doc: self.doc.clone(), base: self.base.clone(), path }
            }
        }

        #[napi]
        impl SvgNode {
            #[doc = " `group`, `path`, `image` or `text`."]
            #[napi(getter)]
            pub fn kind(&self) -> Result<NodeKind> {
                Ok(match self.node()? {
                    usvg::Node::Group(_) => NodeKind::Group,
                    usvg::Node::Path(_) => NodeKind::Path,
                    usvg::Node::Image(_) => NodeKind::Image,
                    usvg::Node::Text(_) => NodeKind::Text,
                })
            }

            #[doc = " The laid-out content of a text node: chunks, spans, resolved"]
            #[doc = " fonts. Null for anything that is not text."]
            #[doc = ""]
            #[doc = " Its presence is what makes the text types map at all -- a node"]
            #[doc = " payload is not a collection, so nothing else nominates them."]
            #[napi]
            pub fn text(&self) -> Result<Option<Text>> {
                Ok(match self.node()? {
                    usvg::Node::Text(t) => Some(Text::wrap((**t).clone(), self.doc())),
                    _ => None,
                })
            }

            #[doc = " The shape of a path node: geometry, fill, stroke, paint order."]
            #[doc = " Null for a group, an image or a text node."]
            #[doc = ""]
            #[doc = " This is what makes `Fill`, `Stroke` and `Path` reachable at all: the"]
            #[doc = " mapper prunes any generated type no exposed method hands out."]
            // napi maps a Rust type *named* `Path` to `string` in the .d.ts --
            // it is reading the name, not the type, and colliding with
            // std::path::PathBuf. The runtime is right either way; this fixes
            // only what gets written into the declarations.
            #[napi(ts_return_type = "Path | null")]
            pub fn path(&self) -> Result<Option<Path>> {
                Ok(match self.node()? {
                    usvg::Node::Path(p) => Some(Path::from(&**p)),
                    _ => None,
                })
            }

            #[doc = " The content of an image node: where it sits, how it is to"]
            #[doc = " be scaled, and the bytes themselves. Null for anything else."]
            #[doc = ""]
            #[doc = " `kind` is a discriminated union. The four raster variants"]
            #[doc = " carry the encoded bytes exactly as the document supplied"]
            #[doc = " them -- usvg says they should be decoded by the caller, and"]
            #[doc = " this is the caller -- while `svg` carries none, an embedded"]
            #[doc = " SVG being a tree rather than a payload."]
            #[napi]
            pub fn image(&self) -> Result<Option<Image>> {
                Ok(match self.node()? {
                    usvg::Node::Image(i) => Some(Image::wrap((**i).clone(), self.doc())),
                    _ => None,
                })
            }

            #[doc = " Direct children. Empty for anything that is not a group."]
            #[napi]
            pub fn children(&self) -> Result<Vec<SvgNode>> {
                let count = match self.node()? {
                    usvg::Node::Group(g) => g.children().len(),
                    _ => 0,
                };
                Ok((0..count).map(|i| self.child(i)).collect())
            }

            #[doc = " Extent of this element's own filters, if it is a filtered group."]
            #[doc = " This is what the document-level `filtersBoundingBox()` cannot see:"]
            #[doc = " usvg wraps every filtered element in an inner group."]
            #[napi]
            pub fn filters_bbox(&self) -> Result<Option<BBox>> {
                Ok(match self.node()? {
                    usvg::Node::Group(g) => g.filters_bounding_box().map(BBox::from),
                    _ => None,
                })
            }

            #[doc = " Extent this element will actually occupy once rendered, contours and"]
            #[doc = " filters included."]
            #[napi]
            pub fn extent(&self) -> Result<Option<BBox>> {
                Ok(node_extent(self.node()?).map(BBox::from))
            }

            #[doc = " Clip path applied to this element, if it is a clipped group."]
            #[doc = ""]
            #[doc = " usvg hands out a bare `&ClipPath` here, with no `Arc` to hold on to,"]
            #[doc = " so it is found in the document's clip-path table by identity."]
            #[napi]
            pub fn clip_path(&self) -> Result<Option<ClipPath>> {
                let usvg::Node::Group(g) = self.node()? else {
                    return Ok(None);
                };
                let Some(target) = g.clip_path() else {
                    return Ok(None);
                };
                let Some(tree) = &self.doc.tree else {
                    return Ok(None);
                };
                Ok(find_def(tree.clip_paths(), target).map(|c| ClipPath::wrap(c, self.doc())))
            }

            #[doc = " Mask applied to this element, found like `clipPath`."]
            #[napi]
            pub fn mask(&self) -> Result<Option<Mask>> {
                let usvg::Node::Group(g) = self.node()? else {
                    return Ok(None);
                };
                let Some(target) = g.mask() else {
                    return Ok(None);
                };
                let Some(tree) = &self.doc.tree else {
                    return Ok(None);
                };
                Ok(find_def(tree.masks(), target).map(|m| Mask::wrap(m, self.doc())))
            }

            #[doc = " The Send half: bytes, no JS handle, so a worker thread can run it."]
            #[async_twin(render_png_async, Buffer)]
            fn png_bytes(&self, params: Option<RenderParams>) -> Result<Vec<u8>> {
                let node = self.node()?;
                self.doc.logs.scope(|| render_node_png(node, &params.unwrap_or_default()))
            }

            #[doc = " Renders this element alone, sized to its own extent."]
            #[napi]
            pub fn render_png(&self, params: Option<RenderParams>) -> Result<Buffer> {
                self.png_bytes(params).map(IntoJs::into_js)
            }

            #node_methods
        }

        #[doc = " A parsed SVG, ready to be rendered any number of times."]
        #[napi]
        #[doc = ""]
        // `Clone` because an async twin captures the receiver: every field is
        // behind an `Arc` but the two short pending lists, so the clone is a
        // few refcount bumps and the worker thread never touches the JS heap.
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
            pending_images: Vec<String>,
            pending_fonts: Vec<String>,
            logs: Logs,
        }

        #[napi]
        impl Resvg {
            #[napi(constructor)]
            pub fn new(
                svg: Either<String, Buffer>,
                options: Option<RenderOptions>,
                fonts: Option<&FontDatabase>,
                images: Option<std::collections::HashMap<String, Buffer>>,
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
                    logs.scope(|| Self::build(&svg, &options, fonts.clone(), &images))?;
                Ok(Self {
                    tree: std::sync::Arc::new(tree),
                    svg: std::sync::Arc::new(svg),
                    options,
                    fonts,
                    images,
                    pending_images,
                    pending_fonts,
                    logs,
                })
            }

            #[doc = " Hrefs of `<image>` elements that neither the supplied buffers nor the"]
            #[doc = " filesystem could resolve. Feed them back with `resolveImage`."]
            #[napi]
            pub fn pending_images(&self) -> Vec<String> {
                self.pending_images.clone()
            }

            #[doc = " Drains what usvg and resvg reported while parsing and rendering"]
            #[doc = " this document, async twins included, and nothing from any other."]
            #[doc = " Collected only once `setLogLevel` has set a level."]
            #[napi(js_name = "takeLogs")]
            pub fn take_own_logs(&self) -> Vec<String> {
                std::mem::take(&mut *self.logs.0.lock().unwrap_or_else(|e| e.into_inner()))
            }

            #[doc = " Named font families requested by `<text>` that the database does not"]
            #[doc = " carry, so the text was rendered with a fallback face. Load them with"]
            #[doc = " `FontDatabase` and re-parse to fix the output."]
            #[napi]
            pub fn pending_fonts(&self) -> Vec<String> {
                self.pending_fonts.clone()
            }

            #[doc = " The face that actually drew a glyph, from the database this"]
            #[doc = " document was parsed with."]
            #[doc = ""]
            #[doc = " `pendingFonts()` names the families that were missing; this names"]
            #[doc = " what was used instead, glyph by glyph, which is the half a"]
            #[doc = " document-wide list cannot answer. A fallback that renders is the"]
            #[doc = " failure nobody sees."]
            #[doc = ""]
            #[doc = " Same answer as `glyph.font`, plus a provenance check: a glyph laid"]
            #[doc = " out against another database comes back `null`. A `fontdb::ID` is"]
            #[doc = " a slotmap key, meaningless outside the database that issued it,"]
            #[doc = " and read against this one it could name an unrelated face."]
            #[napi]
            pub fn face_of(&self, glyph: &PositionedGlyph) -> Option<FontFace> {
                let fonts = glyph.doc.fonts.as_ref()?;
                if !std::sync::Arc::ptr_eq(fonts, &self.fonts) {
                    return None;
                }
                glyph.font()
            }

            #[doc = " Supplies one image and re-parses the document."]
            #[doc = ""]
            #[doc = " Pass everything you already have through the constructor's `images`"]
            #[doc = " argument instead: this re-parses once per call."]
            #[napi]
            pub fn resolve_image(&mut self, href: String, data: Buffer) -> Result<()> {
                std::sync::Arc::make_mut(&mut self.images)
                    .insert(href, std::sync::Arc::new(data.to_vec()));
                let (tree, pending_images, pending_fonts) = self.logs.scope(|| {
                    Self::build(&self.svg, &self.options, self.fonts.clone(), &self.images)
                })?;
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
                self.draw(params.unwrap_or_default())?
                    .encode_png()
                    .map_err(|e| Error::from_reason(format!("PNG encoding failed: {e}")))
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
                svg: Either<String, Buffer>,
                options: Option<RenderOptions>,
                fonts: Option<&FontDatabase>,
                images: Option<std::collections::HashMap<String, Buffer>>,
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

            #[doc = " Bounding box of one element, by `id`, as it will be rendered"]
            #[doc = " (contours, filters and transforms included). `null` if there is no"]
            #[doc = " such id or the element has no visible size."]
            #[napi]
            pub fn node_bbox(&self, id: String) -> Option<BBox> {
                self.tree
                    .node_by_id(&id)
                    .and_then(node_extent)
                    .map(BBox::from)
            }

            #[doc = " Renders a single element, by `id`, into its own PNG -- the output is"]
            #[doc = " sized to that element, not to the document. Useful to slice an icon"]
            #[doc = " out of a sprite sheet."]
            #[napi]
            pub fn render_node_png(
                &self,
                id: String,
                params: Option<RenderParams>,
            ) -> Result<Buffer> {
                self.node_png_bytes(id, params).map(IntoJs::into_js)
            }

            #[doc = " The Send half of `renderNodePng`."]
            #[async_twin(render_node_png_async, Buffer)]
            fn node_png_bytes(
                &self,
                id: String,
                params: Option<RenderParams>,
            ) -> Result<Vec<u8>> {
                let node = self
                    .tree
                    .node_by_id(&id)
                    .ok_or_else(|| Error::from_reason(format!("no element with id {id:?}")))?;
                self.logs.scope(|| render_node_png(node, &params.unwrap_or_default()))
            }

            #[doc = " Handle on one element, by `id`."]
            #[napi]
            pub fn node(&self, id: String) -> Option<SvgNode> {
                path_of_id(self.tree.root(), &id, &mut Vec::new()).map(|path| SvgNode {
                    doc: self.doc(),
                    base: NodeBase::Tree(self.tree.clone()),
                    path,
                })
            }

            #[doc = " Direct children of the root group."]
            #[napi]
            pub fn children(&self) -> Vec<SvgNode> {
                (0..self.tree.root().children().len())
                    .map(|i| SvgNode {
                        doc: self.doc(),
                        base: NodeBase::Tree(self.tree.clone()),
                        path: vec![i],
                    })
                    .collect()
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
            fn doc(&self) -> Doc {
                Doc {
                    tree: Some(self.tree.clone()),
                    fonts: Some(self.fonts.clone()),
                    logs: self.logs.clone(),
                }
            }
        }

        impl Resvg {
            fn build(
                svg: &[u8],
                options: &RenderOptions,
                fonts: std::sync::Arc<usvg::fontdb::Database>,
                images: &std::sync::Arc<ImageMap>,
            ) -> Result<(usvg::Tree, Vec<String>, Vec<String>)> {
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
                let tree = contained("usvg", || usvg::Tree::from_data(svg, &opts))?
                    .map_err(|e| Error::from_reason(format!("invalid SVG: {e}")))?;
                Ok((
                    tree,
                    std::mem::take(&mut *missing_images.0.lock().unwrap_or_else(|e| e.into_inner())),
                    std::mem::take(&mut *missing_fonts.0.lock().unwrap_or_else(|e| e.into_inner())),
                ))
            }

            fn draw(&self, p: RenderParams) -> Result<tiny_skia::Pixmap> {
                self.logs.scope(|| draw(&self.tree, &p))
            }
        }

        #[doc = " Runs a call into usvg/resvg, turning a panic into a JS error. An"]
        #[doc = " unwind reaching napi's extern \"C\" frame aborts the whole process,"]
        #[doc = " and these are the paths an untrusted document drives."]
        fn contained<T>(what: &str, f: impl FnOnce() -> T) -> Result<T> {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .map_err(|_| Error::from_reason(format!("{what} panicked")))
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

        #[doc = " Size, scale and background of one render pass, shared by `draw`"]
        #[doc = " and `render_node_png`."]
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
                let max = p.max_pixels.unwrap_or(268_435_456.0); // 2^28
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

        #[doc = " Shared by `Resvg.renderNodePng` and `SvgNode.renderPng`."]
        fn render_node_png(node: &usvg::Node, p: &RenderParams) -> Result<Vec<u8>> {
                let bbox = node_extent(node)
                    .ok_or_else(|| Error::from_reason("element has no visible size"))?;
                // resvg::render_node renders the node's *local* geometry but
                // offsets by its *absolute* bbox, so both have to be undone:
                // cancel that pre_translate, then supply the ancestor transform.
                let inner = node
                    .abs_layer_bounding_box()
                    .ok_or_else(|| Error::from_reason("element is empty"))?;
                let (scale, mut pixmap) = canvas(bbox.width(), bbox.height(), p)?;
                contained("resvg", || resvg::render_node(
                    node,
                    tiny_skia::Transform::from_scale(scale, scale)
                        .pre_translate(-bbox.x(), -bbox.y())
                        .pre_concat(node.abs_transform())
                        .pre_translate(inner.x(), inner.y()),
                    &mut pixmap.as_mut(),
                ))?
                .ok_or_else(|| Error::from_reason("element is empty"))?;
                pixmap
                    .encode_png()
                    .map_err(|e| Error::from_reason(format!("PNG encoding failed: {e}")))
        }

        #[doc = " Rendered extent of a node."]
        #[doc = ""]
        #[doc = " `Node::abs_layer_bounding_box` gives a Group its full layer box but"]
        #[doc = " hands back only the *geometric* box for a Path or Text, which would"]
        #[doc = " clip the stroke, so widen those here."]
        fn node_extent(node: &usvg::Node) -> Option<usvg::NonZeroRect> {
            match node {
                usvg::Node::Group(g) => Some(g.abs_layer_bounding_box()),
                usvg::Node::Path(p) => p.abs_stroke_bounding_box().to_non_zero_rect(),
                usvg::Node::Text(t) => t.abs_stroke_bounding_box().to_non_zero_rect(),
                usvg::Node::Image(i) => i.abs_bounding_box().to_non_zero_rect(),
            }
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

        impl Task for ParseTask {
            type Output = (usvg::Tree, Vec<String>, Vec<String>);
            type JsValue = Resvg;

            fn compute(&mut self) -> Result<Self::Output> {
                let fonts = self.fonts.get_or_insert_with(default_fontdb).clone();
                self.logs
                    .scope(|| Resvg::build(&self.svg, &self.options, fonts, &self.images))
            }

            fn resolve(&mut self, _: napi::Env, out: Self::Output) -> Result<Self::JsValue> {
                let (tree, pending_images, pending_fonts) = out;
                Ok(Resvg {
                    tree: std::sync::Arc::new(tree),
                    svg: std::sync::Arc::new(std::mem::take(&mut self.svg)),
                    options: std::mem::take(&mut self.options),
                    fonts: self.fonts.take().unwrap_or_else(default_fontdb),
                    images: std::mem::take(&mut self.images),
                    pending_images,
                    pending_fonts,
                    logs: std::mem::take(&mut self.logs),
                })
            }
        }

        // ponytail: an `instanceof` check, which stops a wrong object passed by
        // mistake -- the case the advisory is about -- but not JS that sets an
        // instance's prototype to AbortSignal.prototype on purpose. Code that
        // does that already runs in this process. Goes away with the napi pins:
        // 3.12 fixed the conversion itself.
        #[doc = " An `AbortSignal`, checked before napi converts it. napi 3.11 takes"]
        #[doc = " whatever object it is handed (GHSA-qr54-xrr9-7575) and unwraps its"]
        #[doc = " native pointer as its own, so an instance of one of this crate's"]
        #[doc = " classes passed as `signal` was read as the wrong type."]
        pub struct Signal(AbortSignal);

        impl FromNapiValue for Signal {
            unsafe fn from_napi_value(
                env: napi::sys::napi_env,
                value: napi::sys::napi_value,
            ) -> Result<Self> {
                let mut global = std::ptr::null_mut();
                napi::check_status!(unsafe { napi::sys::napi_get_global(env, &mut global) })?;
                let mut ctor = std::ptr::null_mut();
                napi::check_status!(unsafe {
                    napi::sys::napi_get_named_property(env, global, c"AbortSignal".as_ptr(), &mut ctor)
                })?;
                let mut is = false;
                napi::check_status!(unsafe { napi::sys::napi_instanceof(env, value, ctor, &mut is) })?;
                if !is {
                    return Err(Error::new(Status::InvalidArg, "signal must be an AbortSignal"));
                }
                unsafe { AbortSignal::from_napi_value(env, value) }.map(Signal)
            }
        }

        #[doc = " Parse, rasterise and PNG-encode in one worker-thread round trip."]
        pub struct RenderTask {
            svg: Vec<u8>,
            options: RenderOptions,
            params: RenderParams,
        }

        impl Task for RenderTask {
            type Output = Vec<u8>;
            type JsValue = Buffer;

            fn compute(&mut self) -> Result<Self::Output> {
                let (tree, _, _) = Resvg::build(
                    &self.svg,
                    &self.options,
                    default_fontdb(),
                    &Default::default(),
                )?;
                draw(&tree, &self.params)?
                    .encode_png()
                    .map_err(|e| Error::from_reason(format!("PNG encoding failed: {e}")))
            }

            fn resolve(&mut self, _: napi::Env, png: Self::Output) -> Result<Self::JsValue> {
                Ok(Buffer::from(png))
            }
        }

        #[doc = " One-shot: parse and render a PNG entirely off the event loop."]
        #[doc = ""]
        #[doc = " Uses the process-wide system font database and resolves images off the"]
        #[doc = " filesystem only. For a custom `FontDatabase`, supplied image buffers or"]
        #[doc = " the `pendingImages` / `pendingFonts` reports, use `Resvg.parseAsync`."]
        #[napi(ts_return_type = "Promise<Buffer>")]
        pub fn render_async(
            svg: Either<String, Buffer>,
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
