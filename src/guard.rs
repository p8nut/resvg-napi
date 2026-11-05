//! The resvg-specific guards (ADR 0001, "Safety"; ADR 0002, Q26), written by
//! hand: the generator does not touch this file.
//!
//! The raw API resolves like Rust: usvg's default resolvers read any path an
//! `<image href>` names and decode any image. The providers here are the
//! opt-in that does not:
//!
//! - `guard.ImageHrefResolver`: supplied buffers first, then files under
//!   `resourcesDir` only (canonicalised, `resourcesRoot` widens the fence),
//!   read with `O_NOFOLLOW` and a 256 MiB cap; a raster image whose header
//!   declares more than 2^28 px is refused, and so is gzip data (SVGZ).
//!   Refusals are logged; what stays unresolved is read back with `misses()`.
//! - `guard.FontResolver`: usvg's selectors, noting every named family the
//!   database does not carry (`misses()`).
//!
//! `usvg.Options.imageHrefResolver` and `fontResolver` take them
//! (`Config::providers`). The flat `Resvg` resolves through the same
//! functions until it goes (step 13 of the migration).

use napi::bindgen_prelude::*;
use napi_derive::napi;
use napi_support::Slot;
use napi_support::fs::read_regular;
use resvg::usvg;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Supplied image bytes by href, off the JS heap so a worker thread can read
/// them.
pub(crate) type ImageMap = HashMap<String, Arc<Vec<u8>>>;

/// JS `{ href: Uint8Array }` -> the map the href resolver reads, copied off
/// the JS heap so a worker thread can hold it.
pub(crate) fn image_map(images: Option<HashMap<String, Uint8Array>>) -> ImageMap {
    images
        .unwrap_or_default()
        .into_iter()
        .map(|(href, buf)| (href, Arc::new(buf.to_vec())))
        .collect()
}

/// Hrefs or families no resolver could satisfy, each once, in the order met.
#[derive(Default, Clone)]
pub(crate) struct Misses(Arc<Mutex<Vec<String>>>);

impl Misses {
    /// Records `name`, once: a `<use>` of an image resolves it again.
    /// Poison-tolerant: this mutex only accumulates diagnostics, so a panic
    /// elsewhere must not take the render down with it.
    pub(crate) fn note(&self, name: &str) {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !seen.iter().any(|s| s == name) {
            seen.push(name.to_string());
        }
    }

    /// What was recorded, left in place.
    pub(crate) fn list(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Drains what was recorded.
    pub(crate) fn take(&self) -> Arc<[String]> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner())).into()
    }
}

/// Where an image href may be read from: supplied bytes, and files under a
/// directory.
#[derive(Default)]
pub(crate) struct Fence {
    pub(crate) images: Arc<ImageMap>,
    /// The directory relative hrefs resolve against and files may not leave.
    /// `None`: the parse options' `resources_dir`. Neither: no file is read.
    pub(crate) dir: Option<PathBuf>,
    /// Widens what hrefs may reach to this directory (default: `dir`).
    pub(crate) root: Option<PathBuf>,
}

/// `usvg::ImageHrefResolver` demands `Send + Sync`, so a JS callback cannot
/// live in it. Buffers are handed over up front instead, and whatever stays
/// unresolved is noted in `misses`.
pub(crate) fn href_resolver(fence: Arc<Fence>, misses: Misses) -> usvg::ImageHrefResolver<'static> {
    let sniff = checked(usvg::ImageHrefResolver::default_data_resolver());
    usvg::ImageHrefResolver {
        // Also what a nested SVG's own images go through: usvg forwards
        // `resolve_data` into the sub-document's options.
        resolve_data: checked(usvg::ImageHrefResolver::default_data_resolver()),
        resolve_string: Box::new(move |href: &str, opts: &usvg::Options| {
            // 1. buffer supplied by JS -- let usvg sniff the format itself.
            // One it cannot use (refused, or not an image at all) is a miss
            // like any other, not an image that silently vanishes.
            if let Some(data) = fence.images.get(href)
                && let Some(kind) = sniff("text/plain", data.clone(), opts)
            {
                return Some(kind);
            }
            // 2. a file under the directory, and nowhere else. usvg's default
            // reads any absolute path, and a relative one off the working
            // directory, so an untrusted document could pull local files into
            // its pixels or `toString()` output, or exhaust memory reading
            // `/dev/zero`. Read here rather than through usvg's string
            // resolver, which re-opens the path by name after the checks. Its
            // format rule is kept: SVG by extension, magic otherwise.
            let dir = fence.dir.as_deref().or(opts.resources_dir.as_deref());
            if let Some(path) = under(href, dir, fence.root.as_deref())
                && let Some(data) = read_regular(&path)
            {
                let svg = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
                let mime = if svg { "image/svg+xml" } else { "text/plain" };
                if let Some(kind) = sniff(mime, Arc::new(data), opts) {
                    return Some(kind);
                }
            }
            // 3. give up, but note it
            misses.note(href);
            None
        }),
    }
}

/// Largest raster image accepted, by the size its header declares: 2^28 px.
/// resvg decodes at render time with no limit of its own, so a 1 KB JPEG
/// declaring 65535x65535 would ask for 16 GiB there.
pub(crate) const MAX_IMAGE_PIXELS: u64 = 1 << 28;

/// `resolve` behind the two checks usvg and resvg leave out: gzip data
/// (SVGZ, unsupported, see Cargo.toml) is refused, and so is a raster image
/// whose header declares more than `MAX_IMAGE_PIXELS`. Refusals are logged.
fn checked(
    resolve: usvg::ImageHrefDataResolverFn<'static>,
) -> usvg::ImageHrefDataResolverFn<'static> {
    Box::new(
        move |mime: &str, data: Arc<Vec<u8>>, opts: &usvg::Options| {
            if data.starts_with(&[0x1f, 0x8b]) {
                log::warn!(
                    "{}: image refused, SVGZ is not supported",
                    env!("CARGO_PKG_NAME")
                );
                return None;
            }
            // Fail closed: anything a decoder would treat as raster -- by its mime
            // or by the magic usvg itself sniffs -- needs a size read off its
            // header, or it is not drawn.
            let raster = matches!(
                mime,
                "image/jpg" | "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            ) || matches!(
                imagesize::image_type(&data),
                Ok(imagesize::ImageType::Png
                    | imagesize::ImageType::Jpeg
                    | imagesize::ImageType::Gif
                    | imagesize::ImageType::Webp)
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
        },
    )
}

/// `href` resolved against `dir`, if it lands inside `root` (default: `dir`
/// itself). Canonicalised, so `..` and symlinks cannot step outside it. What
/// the path names is checked by `read_regular`, on the open descriptor.
fn under(href: &str, dir: Option<&Path>, root: Option<&Path>) -> Option<PathBuf> {
    let dir = dir?;
    let root = root.unwrap_or(dir).canonicalize().ok()?;
    let path = dir.join(href).canonicalize().ok()?;
    path.starts_with(&root).then_some(path)
}

/// The resolver is `Send + Sync` too, so instead of calling into JS this wraps
/// the default selector and notes every named family the database does not
/// carry -- i.e. text that silently fell back.
pub(crate) fn font_resolver(misses: Misses) -> usvg::FontResolver<'static> {
    let select = usvg::FontResolver::default_font_selector();
    usvg::FontResolver {
        select_font: Box::new(
            move |font: &usvg::Font, db: &mut Arc<usvg::fontdb::Database>| {
                for family in font.families() {
                    let usvg::FontFamily::Named(name) = family else {
                        continue; // serif/sans-serif/... always map to something
                    };
                    // ponytail: name presence over `faces()`, weight and style
                    // ignored. Switch to `Database::query` if that granularity
                    // ever matters.
                    let known = db
                        .faces()
                        .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)));
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

/// What `new guard.ImageHrefResolver(options)` reads from.
#[napi(
    object,
    object_to_js = false,
    namespace = "guard",
    js_name = "ImageHrefResolverOptions"
)]
#[derive(Default)]
pub struct GuardImageHrefResolverOptions {
    /// Image bytes by href, tried first: an `<image href>` equal to a key
    /// gets its value, whatever the format (sniffed, then checked like a
    /// file).
    pub images: Option<HashMap<String, Uint8Array>>,
    /// The directory relative hrefs resolve against, and the only one files
    /// are read from. Default: the parse options' `resourcesDir`. With
    /// neither, no file is read.
    pub resources_dir: Option<String>,
    /// Widens what hrefs may reach to this directory, an ancestor of
    /// `resourcesDir`, so a template beside a shared assets folder can use
    /// `../assets/logo.png`. Default: `resourcesDir` itself.
    pub resources_root: Option<String>,
}

/// usvg's image href resolution, fenced: the provider for
/// `usvg.Options.imageHrefResolver`.
///
/// - Supplied `images` first, by href.
/// - Then a file under `resourcesDir` (or the parse options'), and nowhere
///   else: canonicalised, so `..`, an absolute path or a symlink cannot leave
///   it (`resourcesRoot` widens the fence); only a regular file, opened
///   without following a final symlink, of at most 256 MiB.
/// - `data:` URLs as usvg reads them.
/// - Whatever the source, gzip data (SVGZ) is refused, and so is a raster
///   image whose header declares more than 2^28 px, or no readable size.
///   Refusals are logged (`setLogLevel('warn')`).
///
/// What stays unresolved is listed by `misses()`. Use one instance per
/// parse: an instance used for several, at once or in turn, lists the misses
/// of them all.
#[napi(namespace = "guard", js_name = "ImageHrefResolver")]
pub struct GuardImageHrefResolver {
    inner: Slot<(Arc<Fence>, Misses)>,
}

#[napi(namespace = "guard")]
impl GuardImageHrefResolver {
    #[napi(constructor)]
    pub fn new(options: Option<GuardImageHrefResolverOptions>) -> Self {
        let o = options.unwrap_or_default();
        let fence = Fence {
            images: Arc::new(image_map(o.images)),
            dir: o.resources_dir.map(PathBuf::from),
            root: o.resources_root.map(PathBuf::from),
        };
        Self {
            inner: Slot::new((Arc::new(fence), Misses::default())),
        }
    }

    /// The hrefs no source resolved, each once, in the order met: during
    /// every parse this instance served so far, async ones included once
    /// they settled.
    #[napi]
    pub fn misses(&self) -> std::result::Result<Vec<String>, Error<String>> {
        Ok(self.inner.get("ImageHrefResolver")?.1.list())
    }

    /// Drops the supplied images now, not at garbage collection: every later
    /// call throws `Disposed`, and so does passing it to a parse. A parse
    /// already running keeps what it holds. Harmless twice.
    #[napi]
    pub fn dispose(&mut self) -> std::result::Result<(), Error<String>> {
        self.inner.dispose("ImageHrefResolver")
    }

    /// Whether `dispose()` has been called.
    #[napi(getter)]
    pub fn disposed(&self) -> bool {
        self.inner.disposed()
    }
}

impl GuardImageHrefResolver {
    /// The resolver a parse uses, sharing this instance's images and misses.
    pub(crate) fn resolver(
        &self,
    ) -> std::result::Result<usvg::ImageHrefResolver<'static>, Error<String>> {
        let (fence, misses) = self.inner.get("ImageHrefResolver")?;
        Ok(href_resolver(fence.clone(), misses.clone()))
    }
}

/// usvg's font selection, recording the named families the database does
/// not carry: the provider for `usvg.Options.fontResolver`. Text in such a
/// family is drawn with a fallback face, or not at all.
///
/// `misses()` lists them. Use one instance per parse: an instance used for
/// several lists the misses of them all.
#[napi(namespace = "guard", js_name = "FontResolver")]
pub struct GuardFontResolver {
    inner: Slot<Misses>,
}

#[napi(namespace = "guard")]
impl GuardFontResolver {
    #[allow(clippy::new_without_default)] // a JS constructor
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Slot::new(Misses::default()),
        }
    }

    /// The named families the database lacked, each once, in the order met:
    /// during every parse this instance served so far, async ones included
    /// once they settled.
    #[napi]
    pub fn misses(&self) -> std::result::Result<Vec<String>, Error<String>> {
        Ok(self.inner.get("FontResolver")?.list())
    }

    /// Every later call throws `Disposed`, and so does passing it to a parse.
    /// Harmless twice.
    #[napi]
    pub fn dispose(&mut self) -> std::result::Result<(), Error<String>> {
        self.inner.dispose("FontResolver")
    }

    /// Whether `dispose()` has been called.
    #[napi(getter)]
    pub fn disposed(&self) -> bool {
        self.inner.disposed()
    }
}

impl GuardFontResolver {
    /// The resolver a parse uses, sharing this instance's misses.
    pub(crate) fn resolver(
        &self,
    ) -> std::result::Result<usvg::FontResolver<'static>, Error<String>> {
        Ok(font_resolver(self.inner.get("FontResolver")?.clone()))
    }
}
