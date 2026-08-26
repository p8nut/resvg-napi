//! Generates the whole NAPI binding layer from the *actual* resvg / usvg /
//! fontdb sources: `syn` parses them, `quote` re-emits Rust.
//!
//! Layout of this file:
//!   1. source discovery  (env override -> git submodule -> cargo registry)
//!   2. AST extraction    (usvg::Options, its enums, fontdb::Database, resvg::render)
//!   3. type mapping      (Rust -> napi/JS)
//!   4. code emission     ($OUT_DIR/bindings.rs, optionally src/generated.rs)

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Fields, ImplItem, Item, ReturnType, Type, Visibility};

/// The codegen's own report: what it derived from upstream, and what it left
/// alone with the reason. Cargo gives a build script no channel but
/// `cargo::warning`, so this competes with real warnings -- clippy's four were
/// buried under 101 lines of it. Off unless asked for.
macro_rules! report {
    ($($arg:tt)*) => {
        if env::var_os("RESVG_NAPI_CODEGEN_LOG").is_some() {
            println!("cargo::warning={}", format!($($arg)*));
        }
    };
}

// ---------------------------------------------------------------------------
// 1. source discovery
// ---------------------------------------------------------------------------

/// Exact versions picked by the resolver, straight out of our own Cargo.lock.
/// Cheap hand-rolled scan: no serde/toml build-dependency needed.
fn locked_versions() -> BTreeMap<String, String> {
    let lock = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("Cargo.lock");
    println!("cargo::rerun-if-changed={}", lock.display());
    let mut out = BTreeMap::new();
    let Ok(text) = fs::read_to_string(&lock) else {
        return out;
    };
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("version = ") {
            if let Some(n) = name.take() {
                // Keep the highest version if a crate appears twice in the graph.
                let v = v.trim_matches('"').to_string();
                let better = out
                    .get(&n)
                    .is_none_or(|old: &String| semver_key(&v) > semver_key(old));
                if better {
                    out.insert(n, v);
                }
            }
        }
    }
    out
}

fn semver_key(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn cargo_home() -> PathBuf {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").expect("HOME unset")).join(".cargo"))
}

/// `$CARGO_HOME/registry/src/<any-index>/<pkg>-<version>/src`
fn registry_src(pkg: &str, want: Option<&str>) -> Option<PathBuf> {
    let mut best: Option<((u64, u64, u64), PathBuf)> = None;
    for index in fs::read_dir(cargo_home().join("registry/src"))
        .ok()?
        .flatten()
    {
        let Ok(entries) = fs::read_dir(index.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let dir = e.file_name().to_string_lossy().to_string();
            let Some(ver) = dir.strip_prefix(&format!("{pkg}-")) else {
                continue;
            };
            // "usvg-0.48.1" is ours, "usvg-parser-0.44.0" is not.
            if !ver.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let src = e.path().join("src");
            if want == Some(ver) {
                return Some(src);
            }
            let key = semver_key(ver);
            if best.as_ref().is_none_or(|(b, _)| key > *b) {
                best = Some((key, src));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Resolution order, most explicit first:
///   1. `<PKG>_SRC_DIR` env var (vendored checkout / git submodule anywhere)
///   2. `vendor/resvg/crates/<pkg>/src` (the upstream monorepo as a submodule)
///   3. cargo registry cache, exact locked version
///   4. cargo registry cache, newest version present
fn locate(pkg: &str, marker: &str, locked: &BTreeMap<String, String>) -> PathBuf {
    let key = format!("{}_SRC_DIR", pkg.to_uppercase().replace('-', "_"));
    println!("cargo::rerun-if-env-changed={key}");
    if let Some(dir) = env::var_os(&key) {
        let p = PathBuf::from(dir);
        assert!(
            p.join(marker).exists(),
            "{key} points at {}, but {marker} is missing there",
            p.display()
        );
        return p;
    }

    let vendored = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("vendor/resvg/crates")
        .join(pkg)
        .join("src");
    if vendored.join(marker).exists() {
        return vendored;
    }

    let locked_ver = locked.get(pkg).map(String::as_str);
    registry_src(pkg, locked_ver)
        .or_else(|| registry_src(pkg, None))
        .filter(|p| p.join(marker).exists())
        .unwrap_or_else(|| {
            panic!(
                "cannot find the sources of `{pkg}` (looked for {marker}).\n\
                 Run `cargo fetch` first, or point {key} at a checkout."
            )
        })
}

fn parse(path: &Path) -> syn::File {
    println!("cargo::rerun-if-changed={}", path.display());
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: parse error: {e}", path.display()))
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// Normalised textual form of a type, e.g. `Option<std::path::PathBuf>`.
fn ty_str(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

// ---------------------------------------------------------------------------
// 2 + 3. usvg::Options -> #[napi(object)] RenderOptions
// ---------------------------------------------------------------------------

struct Field {
    decl: TokenStream,   // the generated `pub name: Option<T>,`
    assign: TokenStream, // `if let Some(v) = ... { o.name = ... }`
}

fn field(ident: &syn::Ident, doc: &[TokenStream], jsty: TokenStream, assign: TokenStream) -> Field {
    Field {
        decl: quote! { #(#doc)* pub #ident: Option<#jsty>, },
        assign,
    }
}

/// Length of a `static NAME: &[T] = &[...]` array. usvg indexes `POW_VEC` with
/// the precision fields *unchecked*, so the bound has to come from upstream
/// rather than from a magic number here.
/// What a Rust type becomes on the JS side.
///
/// Recognition of Rust types lives here, once. The field mapper and the method
/// mapper then each decide how to *emit* a given `Js` -- an assignment in one
/// case, a wrapper body in the other. Before this, "Rect means BBox" was known
/// only to returns and "Vec<String>" only to fields.
#[derive(Clone, PartialEq, Debug)]
enum Js {
    F32,
    F64,
    U32,
    U8,
    Bool,
    Count,
    Str,
    OptStr,
    OptPath,
    StrList,
    Bytes,
    Size,
    Bbox,
    OptBbox,
    TryUnit,
    Enum,
    Matrix,
    // A newtype over f32 with a `get(&self) -> f32`, e.g. `Opacity`.
    Scalar,
    // `Arc<T>`: shareable, so a wrapper class can own one.
    Handle(String),
    // `&[Arc<T>]`
    HandleList(String),
    // A plain data type: all of its members map, so it becomes #[napi(object)].
    Object(String),
    ObjectList(String),
    // A data type that maps only partially: read-only class, never rebuilt.
    Value(String),
    ValueList(String),
    OptObject(String),
    OptValue(String),
    // A tuple newtype over an integer, e.g. `Weight(pub u16)`.
    IntNewtype(String),
}

/// Everything the classifier needs to know about the upstream crates, all of it
/// derived: enums worth mirroring, `f32` newtypes, and `pub type` aliases.
#[derive(Default)]
struct Vocab {
    enums: BTreeSet<String>,
    scalars: BTreeSet<String>,
    aliases: BTreeMap<String, String>,
    ints: BTreeSet<String>,
    objects: BTreeSet<String>,
    values: BTreeSet<String>,
}

impl Vocab {
    /// `Opacity` -> `NormalizedF32` -> ... until it stops being an alias.
    fn resolve(&self, ty: &str) -> String {
        let mut t = ty.to_string();
        for _ in 0..8 {
            match self.aliases.get(&t) {
                Some(next) if *next != t => t = next.clone(),
                _ => break,
            }
        }
        t
    }

    fn classify(&self, ty: &str) -> Option<Js> {
        let t = self.resolve(ty);
        if let Some(js) = classify(&t, &self.enums, &self.scalars) {
            return Some(js);
        }
        if self.ints.contains(&t) {
            return Some(Js::IntNewtype(t));
        }
        classify_object(&t, &self.objects).or_else(|| match classify_object(&t, &self.values) {
            Some(Js::Object(x)) => Some(Js::Value(x)),
            Some(Js::ObjectList(x)) => Some(Js::ValueList(x)),
            Some(Js::OptObject(x)) => Some(Js::OptValue(x)),
            _ => None,
        })
    }
}

/// `pub type X = Y;` pairs, so an alias can be followed to the real type.
fn type_aliases(files: &[syn::File]) -> BTreeMap<String, String> {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(|i| match i {
            Item::Type(t) if is_pub(&t.vis) => Some((t.ident.to_string(), ty_str(&t.ty))),
            _ => None,
        })
        .collect()
}

/// Parses every `.rs` under a crate's `src`, for crates we only mine for
/// vocabulary (newtypes, aliases) rather than wrap.
fn parse_crate(dir: &Path) -> Vec<(PathBuf, syn::File)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let f = parse(&p);
            (p, f)
        })
        .collect()
}

fn classify(ty: &str, known_enums: &BTreeSet<String>, scalars: &BTreeSet<String>) -> Option<Js> {
    Some(match ty {
        "Transform" => Js::Matrix,
        "f32" => Js::F32,
        "f64" => Js::F64,
        "u32" => Js::U32,
        "u8" => Js::U8,
        "bool" => Js::Bool,
        "usize" => Js::Count,
        "String" | "&str" => Js::Str,
        "Option<String>" => Js::OptStr,
        "Option<std::path::PathBuf>" | "Option<PathBuf>" => Js::OptPath,
        "Vec<String>" => Js::StrList,
        "Vec<u8>" | "&[u8]" => Js::Bytes,
        "Size" | "usvg::Size" => Js::Size,
        // geometry newtypes over four f32 with public accessors
        "Rect" | "NonZeroRect" => Js::Bbox,
        "Option<NonZeroRect>" => Js::OptBbox,
        s if s.starts_with("Result<(),") => Js::TryUnit,
        // Arc-held definitions can be owned by a wrapper class.
        s if s.starts_with("&[Arc<") && s.ends_with(">]") => {
            Js::HandleList(s[6..s.len() - 2].to_string())
        }
        s if s.starts_with("&Arc<") && s.ends_with('>') => {
            Js::Handle(s[5..s.len() - 1].to_string())
        }
        s if s.starts_with("Arc<") && s.ends_with('>') => Js::Handle(s[4..s.len() - 1].to_string()),
        other if known_enums.contains(other) => Js::Enum,
        other if scalars.contains(other) => Js::Scalar,
        _ => return None,
    })
}

/// Second classification pass, for types that need the object registry: it is
/// built after the first pass, so it cannot live in `classify`.
fn classify_object(ty: &str, objects: &BTreeSet<String>) -> Option<Js> {
    let bare = ty.trim_start_matches('&');
    if let Some(inner) = bare
        .strip_prefix("[")
        .and_then(|s| s.strip_suffix("]"))
        .or_else(|| bare.strip_prefix("Vec<").and_then(|s| s.strip_suffix(">")))
    {
        let inner = inner.trim_start_matches('&');
        if objects.contains(inner) {
            return Some(Js::ObjectList(inner.to_string()));
        }
    }
    // `impl Iterator<Item = &FaceInfo> + '_`
    if let Some(rest) = bare.strip_prefix("implIterator<Item=") {
        let inner = rest.split('>').next()?.trim_start_matches('&');
        if objects.contains(inner) {
            return Some(Js::ObjectList(inner.to_string()));
        }
    }
    if let Some(inner) = bare
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .map(|s| s.trim_start_matches('&'))
    {
        if objects.contains(inner) {
            return Some(Js::OptObject(inner.to_string()));
        }
    }
    if objects.contains(bare) {
        return Some(Js::Object(bare.to_string()));
    }
    None
}

/// Tuple newtypes over an integer, e.g. `pub struct Weight(pub u16)`.
fn int_newtypes(files: &[syn::File]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Struct(s) = item else { continue };
        if !is_pub(&s.vis) {
            continue;
        }
        if let Fields::Unnamed(u) = &s.fields {
            if u.unnamed.len() == 1 {
                let f = &u.unnamed[0];
                if is_pub(&f.vis)
                    && matches!(
                        ty_str(&f.ty).as_str(),
                        "u8" | "u16" | "u32" | "i32" | "usize"
                    )
                {
                    out.insert(s.ident.to_string());
                }
            }
        }
    }
    out
}

/// Types that are a newtype over `f32`, spotted by their `get(&self) -> f32`.
/// Derived, so `Opacity`, `PositiveF32`, `StrokeWidth`... all come along.
fn f32_newtypes(files: &[syn::File]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() {
            continue;
        }
        let name = ty_str(&imp.self_ty);
        let hit = imp.items.iter().any(|it| match it {
            ImplItem::Fn(f) => {
                is_pub(&f.vis)
                    && f.sig.ident == "get"
                    && matches!(&f.sig.output, ReturnType::Type(_, t) if ty_str(t) == "f32")
            }
            _ => false,
        });
        if hit {
            out.insert(name);
        }
    }
    out
}

/// Every type named by a return of the impl blocks we wrap. Drives which enums
/// get mirrored, so the set follows the wrapped surface instead of a list.
fn returned_types(files: &[syn::File], types: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        // An empty filter means "every impl": used for enum discovery, so an
        // accessor on a generated wrapper class gets its enum mirrored too.
        if imp.trait_.is_some()
            || (!types.is_empty()
                && !types.contains(&ty_str(&imp.self_ty).rsplit("::").next().unwrap_or_default()))
        {
            continue;
        }
        for it in &imp.items {
            if let ImplItem::Fn(f) = it {
                if let (true, ReturnType::Type(_, t)) = (is_pub(&f.vis), &f.sig.output) {
                    out.insert(ty_str(t));
                }
            }
        }
    }
    out
}

fn static_array_len(files: &[syn::File], name: &str) -> usize {
    let s = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Static(s) if s.ident == name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("usvg: `static {name}` not found"));
    let expr = match &*s.expr {
        syn::Expr::Reference(r) => &*r.expr,
        other => other,
    };
    match expr {
        syn::Expr::Array(a) => a.elems.len(),
        _ => panic!("usvg: `{name}` is no longer an array literal"),
    }
}

fn struct_fields_opt<'a>(files: &'a [syn::File], ty: &str) -> Option<&'a Fields> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(&s.fields),
        _ => None,
    })
}

fn struct_fields<'a>(files: &'a [syn::File], ty: &str) -> &'a syn::FieldsNamed {
    let s = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("usvg: `pub struct {ty}` not found"));
    match &s.fields {
        Fields::Named(named) => named,
        _ => panic!("usvg::{ty} is no longer a struct with named fields"),
    }
}

/// Every type name a config struct mentions. Drives which enums get mirrored,
/// so the generated surface follows the struct instead of a hand-kept list.
fn struct_field_types(files: &[syn::File], ty: &str) -> BTreeSet<String> {
    struct_fields(files, ty)
        .named
        .iter()
        .filter(|f| is_pub(&f.vis))
        .map(|f| ty_str(&f.ty))
        .collect()
}

/// Maps one flat config struct to `#[napi(object)]` fields plus the code that
/// writes them back onto the real Rust struct. Used for both `usvg::Options`
/// and `usvg::WriteOptions`.
fn map_struct(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    precision_max: u32,
) -> (Vec<Field>, Vec<String>) {
    let named = struct_fields(files, ty);

    let mut out = Vec::new();
    let mut skipped = Vec::new();

    for f in &named.named {
        if !is_pub(&f.vis) {
            continue;
        }
        let ident = f.ident.clone().unwrap();
        let name = ident.to_string();
        let ty = ty_str(&f.ty);
        let doc = docs(&f.attrs);

        // Every field becomes optional so JS can pass a partial object.
        let mapped = match vocab.classify(&ty) {
            Some(Js::F32) => Some(field(
                &ident,
                &doc,
                quote!(f64),
                quote! { if let Some(v) = self.#ident { o.#ident = v as f32; } },
            )),
            Some(Js::F64) => Some(field(
                &ident,
                &doc,
                quote!(f64),
                quote! { if let Some(v) = self.#ident { o.#ident = v; } },
            )),
            Some(Js::U32) => Some(field(
                &ident,
                &doc,
                quote!(u32),
                quote! { if let Some(v) = self.#ident { o.#ident = v; } },
            )),
            Some(Js::U8) => {
                // A `*_precision` field indexes usvg's POW_VEC without a bounds
                // check, so clamp it to that table's length. Any other u8 just
                // saturates instead of wrapping.
                let max = if name.ends_with("_precision") {
                    precision_max
                } else {
                    u8::MAX as u32
                };
                Some(field(
                    &ident,
                    &doc,
                    quote!(u32),
                    quote! { if let Some(v) = self.#ident { o.#ident = v.min(#max) as u8; } },
                ))
            }
            Some(Js::Bool) => Some(field(
                &ident,
                &doc,
                quote!(bool),
                quote! { if let Some(v) = self.#ident { o.#ident = v; } },
            )),
            Some(Js::Str) => Some(field(
                &ident,
                &doc,
                quote!(String),
                quote! { if let Some(v) = &self.#ident { o.#ident = v.clone(); } },
            )),
            Some(Js::OptStr) => Some(field(
                &ident,
                &doc,
                quote!(String),
                quote! { o.#ident = self.#ident.clone(); },
            )),
            Some(Js::OptPath) => Some(field(
                &ident,
                &doc,
                quote!(String),
                quote! { o.#ident = self.#ident.as_deref().map(std::path::PathBuf::from); },
            )),
            Some(Js::StrList) => Some(field(
                &ident,
                &doc,
                quote!(Vec<String>),
                quote! { if let Some(v) = &self.#ident { o.#ident = v.clone(); } },
            )),
            Some(Js::Bbox) | Some(Js::OptBbox) => Some(field(
                &ident,
                &doc,
                quote!(BBox),
                quote! { if let Some(v) = self.#ident {
                    if let Some(r) = usvg::NonZeroRect::from_xywh(
                        v.x as f32, v.y as f32, v.width as f32, v.height as f32) {
                        o.#ident = r;
                    }
                } },
            )),
            Some(Js::Enum) => {
                let e = format_ident!("{}", vocab.resolve(&ty));
                Some(field(
                    &ident,
                    &doc,
                    quote!(#e),
                    quote! { if let Some(v) = self.#ident { o.#ident = v.into(); } },
                ))
            }
            // Size is opaque with a fallible constructor: flatten to a pair.
            Some(Js::Size) => {
                let w = format_ident!("{}_width", name);
                let h = format_ident!("{}_height", name);
                out.push(Field {
                    decl: quote! {
                        #(#doc)* pub #w: Option<f64>,
                        #(#doc)* pub #h: Option<f64>,
                    },
                    assign: quote! {
                        if let (Some(w), Some(h)) = (self.#w, self.#h) {
                            if let Some(s) = usvg::Size::from_wh(w as f32, h as f32) {
                                o.#ident = s;
                            }
                        }
                    },
                });
                continue;
            }
            // Vocabulary entries that only mean something on a method signature.
            Some(Js::Scalar) => Some(field(
                &ident,
                &doc,
                quote!(f64),
                quote! { if let Some(v) = self.#ident { o.#ident = (v as f32).into(); } },
            )),
            // Handles are class instances: they belong on a method, not in a
            // plain JSON object.
            Some(Js::IntNewtype(_)) => Some(field(
                &ident,
                &doc,
                quote!(u32),
                quote! { if let Some(v) = self.#ident { o.#ident.0 = v as _; } },
            )),
            // Class instances and nested objects belong on methods, not in a
            // flat config object.
            Some(Js::Bytes)
            | Some(Js::Count)
            | Some(Js::TryUnit)
            | Some(Js::Matrix)
            | Some(Js::Handle(_))
            | Some(Js::HandleList(_))
            | Some(Js::Object(_))
            | Some(Js::ObjectList(_))
            | Some(Js::Value(_))
            | Some(Js::ValueList(_))
            | Some(Js::OptObject(_))
            | Some(Js::OptValue(_))
            | None => None,
        };

        if let Some(m) = mapped {
            out.push(m);
            continue;
        }

        // --- not transposable as data. Two of them are wired by the emitter
        // template instead (opaque class / href hook); the rest is dropped.
        const HANDLED: [&str; 3] = ["fontdb", "image_href_resolver", "font_resolver"];
        if HANDLED.contains(&name.as_str()) {
            skipped.push(format!("{name}: {ty} (handled manually by the template)"));
        } else {
            skipped.push(format!("{name}: {ty} (dropped)"));
        }
    }

    (out, skipped)
}

fn docs(attrs: &[syn::Attribute]) -> Vec<TokenStream> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .map(|a| quote!(#a))
        .collect()
}

/// Only the enums actually referenced by `Options` get mirrored — the SVG tree
/// enums (Paint, Node, ...) carry payloads and stay on the Rust side.
/// Public module names declared anywhere in the crate (`pub mod filter;`).
/// A type defined in `filter.rs` is then reachable as `usvg::filter::X`.
fn public_modules(files: &[syn::File]) -> BTreeSet<String> {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(|i| match i {
            Item::Mod(m) if is_pub(&m.vis) => Some(m.ident.to_string()),
            _ => None,
        })
        .collect()
}

/// Upstream path of each type name, derived from the file it lives in.
fn upstream_modules(
    parsed: &[(PathBuf, syn::File)],
    modules: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (path, file) in parsed {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let prefix = if modules.contains(stem) {
            stem.to_string()
        } else {
            String::new()
        };
        for item in &file.items {
            let name = match item {
                Item::Enum(e) if is_pub(&e.vis) => e.ident.to_string(),
                Item::Struct(s) if is_pub(&s.vis) => s.ident.to_string(),
                _ => continue,
            };
            out.entry(name).or_insert_with(|| prefix.clone());
        }
    }
    out
}

/// `usvg::filter::EdgeMode` or `usvg::BlendMode`, as appropriate.
fn upstream_path(
    name: &str,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> TokenStream {
    let ident = format_ident!("{}", name);
    match modules.get(name) {
        Some(m) if !m.is_empty() => {
            let m = format_ident!("{}", m);
            quote!(#root::#m::#ident)
        }
        _ => quote!(#root::#ident),
    }
}

fn map_enums(
    files: &[syn::File],
    wanted: &BTreeSet<String>,
    modules: &BTreeMap<String, String>,
) -> (BTreeSet<String>, TokenStream) {
    let mut names = BTreeSet::new();
    let mut code = TokenStream::new();

    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Enum(e) = item else { continue };
        if !is_pub(&e.vis) || !wanted.contains(&e.ident.to_string()) {
            continue;
        }
        if !e.variants.iter().all(|v| matches!(v.fields, Fields::Unit)) {
            continue; // enum with payload -> no clean JS representation
        }
        let ident = &e.ident;
        let variants: Vec<_> = e.variants.iter().map(|v| v.ident.clone()).collect();
        let doc = docs(&e.attrs);
        let up = upstream_path(&ident.to_string(), modules, &quote!(usvg));
        names.insert(ident.to_string());
        code.extend(quote! {
            #(#doc)*
            #[napi(string_enum = "camelCase")]
            #[derive(Copy, Clone)]
            pub enum #ident { #(#variants,)* }

            impl From<#ident> for #up {
                fn from(v: #ident) -> Self {
                    match v { #(#ident::#variants => <#up>::#variants,)* }
                }
            }

            impl From<#up> for #ident {
                fn from(v: #up) -> Self {
                    match v { #(<#up>::#variants => #ident::#variants,)* }
                }
            }
        });
    }

    (names, code)
}

// ---------------------------------------------------------------------------
// 2 + 3. fontdb::Database -> opaque #[napi] class
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Arg {
    Str,
    PathStr,
    Bytes,
    // fontdb's `ID` is an opaque slotmap key with no JS form. A `FontFace`
    // carries the `FaceInfo` it came from, so it can stand in for the key.
    Face,
}

enum Ret {
    Unit,
    Bool,
    Count,
    TryUnit,
    Text,
    Box_,
    OptBox,
    Matrix,
    Scalar,
    Dims,
    Enum(proc_macro2::Ident),
    Handle(String),
    HandleList(String),
    Num,
    Int,
    Object(String),
    ObjectList(String),
    Value(String),
    ValueList(String),
    OptObject(String),
    OptValue(String),
    IntNewtype,
}

/// Wraps every `pub fn` of `impl <ty>` whose whole signature is transposable.
/// `receiver` is the expression the call hangs off, so the same pass serves
/// `fontdb::Database`, `usvg::Tree` and the root `usvg::Group`.
/// `impl Deref for X { type Target = Y }` -> Y. Upstream uses it as inheritance
/// (`LinearGradient` derefs to `BaseGradient`), and a per-impl scan cannot see
/// through it, so the chain has to be walked explicitly.
fn deref_target(files: &[syn::File], ty: &str) -> Option<String> {
    files.iter().flat_map(|f| &f.items).find_map(|i| {
        let Item::Impl(imp) = i else { return None };
        // syn 3: `trait_` is `(Path, Token![for])`; in syn 2 the path sat at `.1`
        // behind the negative-impl bang, which has moved into `modifiers`.
        let (trait_path, _) = imp.trait_.as_ref()?;
        let trait_name = quote!(#trait_path).to_string().replace(' ', "");
        if trait_name.rsplit("::").next() != Some("Deref")
            || ty_str(&imp.self_ty).rsplit("::").next() != Some(ty)
        {
            return None;
        }
        imp.items.iter().find_map(|it| match it {
            ImplItem::Type(t) if t.ident == "Target" => Some(ty_str(&t.ty)),
            _ => None,
        })
    })
}

/// Emits `#[napi(object)] pub struct X { .. }` plus `From<&upstream>` for a plain
/// data type: accessors first, public fields if it has no accessors. Members
/// that do not map are dropped -- the object is deliberately partial.
/// One mappable member of a data type: its JS name, its JS type, and the
/// expression that produces it from a binding named `v`.
struct Member {
    id: proc_macro2::Ident,
    jsty: TokenStream,
    value: TokenStream,
}

/// Walks a data type's members -- accessors first, public fields otherwise --
/// and maps the ones it can. Shared by the strict object emitter and the value
/// class emitter, which differ only in how they package the result.
fn data_members(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
) -> (Vec<Member>, Vec<String>, BTreeSet<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut reached = BTreeSet::new();

    let mut push = |name: &str, ty_s: &str, access: TokenStream| {
        let id = format_ident!("{}", name);
        let (jsty, value) = match vocab.classify(ty_s) {
            Some(Js::F32) | Some(Js::F64) => (quote!(f64), quote!(#access as f64)),
            Some(Js::U32) | Some(Js::U8) => (quote!(u32), quote!(#access as u32)),
            Some(Js::Bool) => (quote!(bool), quote!(#access)),
            Some(Js::Count) => (quote!(u32), quote!(#access as u32)),
            Some(Js::Str) => (quote!(String), quote!(#access.to_string())),
            Some(Js::Scalar) => (quote!(f64), quote!(#access.get() as f64)),
            Some(Js::IntNewtype(_)) => (quote!(u32), quote!(#access.0 as u32)),
            Some(Js::Bbox) => (quote!(BBox), quote!(BBox::from(#access))),
            Some(Js::Matrix) => (quote!(Matrix), quote!(Matrix::from(#access))),
            Some(Js::Enum) => {
                let e = format_ident!("{}", vocab.resolve(ty_s));
                (quote!(#e), quote!(#e::from(#access)))
            }
            Some(Js::Object(t)) => {
                let o = format_ident!("{}", t);
                reached.insert(t.clone());
                let arg = if ty_s.starts_with('&') {
                    quote!(#access)
                } else {
                    quote!(&#access)
                };
                (quote!(#o), quote!(#o::from(#arg)))
            }
            Some(Js::ObjectList(t)) => {
                let o = format_ident!("{}", t);
                reached.insert(t.clone());
                (
                    quote!(Vec<#o>),
                    quote!(#access.iter().map(#o::from).collect()),
                )
            }
            _ => {
                // `Option<T>` where T maps stays optional on the JS side.
                let raw = ty_s
                    .trim_start_matches('&')
                    .strip_prefix("Option<")
                    .and_then(|s| s.strip_suffix('>'));
                let is_ref = raw.is_some_and(|s| s.starts_with('&'));
                let inner = raw.map(|s| s.trim_start_matches('&').to_string());
                match inner
                    .as_deref()
                    .and_then(|i| vocab.classify(i).map(|j| (i, j)))
                {
                    Some((_, Js::F32)) | Some((_, Js::F64)) => {
                        out.push(Member {
                            id,
                            jsty: quote!(Option<f64>),
                            value: quote!(#access.map(|x| x as f64)),
                        });
                        return;
                    }
                    Some((_, Js::Scalar)) => {
                        out.push(Member {
                            id,
                            jsty: quote!(Option<f64>),
                            value: quote!(#access.map(|x| x.get() as f64)),
                        });
                        return;
                    }
                    Some((i, Js::Object(_))) => {
                        let o = format_ident!("{}", i);
                        reached.insert(i.to_string());
                        let value = if is_ref {
                            quote!(#access.map(#o::from))
                        } else {
                            quote!(#access.map(|x| #o::from(&x)))
                        };
                        out.push(Member {
                            id,
                            jsty: quote!(Option<#o>),
                            value,
                        });
                        return;
                    }
                    _ => {
                        skipped.push(format!("{name}: {ty_s}"));
                        return;
                    }
                }
            }
        };
        out.push(Member { id, jsty, value });
    };

    let mut has_accessors = false;
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || ty_str(&imp.self_ty).rsplit("::").next() != Some(ty) {
            continue;
        }
        for it in &imp.items {
            let ImplItem::Fn(f) = it else { continue };
            if !is_pub(&f.vis) || f.sig.inputs.len() != 1 {
                continue;
            }
            let ReturnType::Type(_, o) = &f.sig.output else {
                continue;
            };
            let id = &f.sig.ident;
            has_accessors = true;
            push(&id.to_string(), &ty_str(o), quote!(v.#id()));
        }
    }
    if !has_accessors {
        if let Some(Fields::Named(named)) = struct_fields_opt(files, ty) {
            for f in &named.named {
                if !is_pub(&f.vis) {
                    continue;
                }
                let id = f.ident.clone().unwrap();
                push(&id.to_string(), &ty_str(&f.ty), quote!(v.#id.clone()));
            }
        }
    }
    (out, skipped, reached)
}

/// Strict `#[napi(object)]`: emitted only when *every* member maps, so the value
/// is a faithful copy that could also be rebuilt.
fn object_struct(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> (TokenStream, Vec<String>, BTreeSet<String>) {
    let (members, skipped, reached) = data_members(files, ty, vocab);
    if members.is_empty() || !skipped.is_empty() {
        return (TokenStream::new(), skipped, reached);
    }
    let name = format_ident!("{}", ty);
    let up = upstream_path(ty, modules, root);
    let decls = members.iter().map(|m| {
        let (id, t) = (&m.id, &m.jsty);
        quote!(pub #id: #t,)
    });
    let inits = members.iter().map(|m| {
        let (id, e) = (&m.id, &m.value);
        quote!(#id: #e,)
    });
    let doc = format!(" Plain view of a `{ty}`.");
    let code = quote! {
        #[doc = #doc]
        #[napi(object)]
        #[derive(Clone)]
        pub struct #name {
            #(#decls)*
        }

        impl From<&#up> for #name {
            fn from(v: &#up) -> Self {
                Self { #(#inits)* }
            }
        }
    };
    (code, skipped, reached)
}

/// A data type that does *not* map completely becomes a read-only class instead:
/// it keeps the upstream value, exposes what maps as getters, and never claims
/// to be reconstructible. `FaceInfo` is the case that matters -- its `ID`,
/// `Source`, `Style` and `Stretch` members have no JS form.
fn value_class(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> (TokenStream, Vec<String>, BTreeSet<String>) {
    let (members, skipped, reached) = data_members(files, ty, vocab);
    if members.is_empty() {
        return (TokenStream::new(), skipped, reached);
    }
    let name = value_class_ident(ty);
    let up = upstream_path(ty, modules, root);
    let getters = members.iter().map(|m| {
        let (id, t, e) = (&m.id, &m.jsty, &m.value);
        quote! {
            #[napi(getter)]
            pub fn #id(&self) -> #t {
                let v = &self.inner;
                #e
            }
        }
    });
    let doc = format!(" Read-only view of a `{ty}`.");
    let code = quote! {
        #[doc = #doc]
        #[napi]
        pub struct #name {
            inner: #up,
        }

        impl #name {
            fn wrap(inner: #up) -> Self {
                Self { inner }
            }
        }

        #[napi]
        impl #name {
            #(#getters)*
        }
    };
    (code, skipped, reached)
}

/// Naming decision: `FaceInfo` reads as `FontFace` on the JS side.
fn value_class_ident(ty: &str) -> proc_macro2::Ident {
    format_ident!("{}", if ty == "FaceInfo" { "FontFace" } else { ty })
}

/// JS class name for an Arc-held usvg definition: `filter::Filter` -> `Filter`.
fn wrapper_ident(ty: &str) -> proc_macro2::Ident {
    let last = ty.rsplit("::").next().unwrap_or(ty);
    // `fontdb::Database` already has a hand-written class.
    let name = if ty == "fontdb::Database" {
        "FontDatabase"
    } else {
        last
    };
    format_ident!("{}", name)
}

/// Rust path of that definition, as reachable from our crate.
fn wrapper_path(ty: &str, modules: &BTreeMap<String, String>) -> TokenStream {
    if ty.contains("::") {
        let segs: Vec<_> = ty.split("::").map(|s| format_ident!("{}", s)).collect();
        quote!(usvg::#(#segs)::*)
    } else {
        upstream_path(ty, modules, &quote!(usvg))
    }
}

/// Emits `#[napi] pub struct T { inner: Arc<...> }` plus its read-only
/// accessors, and reports the handle types those accessors reach in turn.
fn wrapper_class(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
) -> (TokenStream, Vec<String>, BTreeSet<String>) {
    let name = wrapper_ident(ty);
    let path = wrapper_path(ty, modules);
    // Walk the type itself, then whatever it derefs to, sharing one name set so
    // an override on the outer type wins.
    let mut taken = BTreeSet::new();
    let mut methods = TokenStream::new();
    let mut skipped = Vec::new();
    let mut current = Some(ty.rsplit("::").next().unwrap_or(ty).to_string());
    let inner = quote!(self.inner);
    for _ in 0..4 {
        let Some(t) = current.take() else { break };
        let (code, sk) = map_methods(
            &MethodPass {
                files,
                ty: &t,
                receiver: &inner,
                skip: &[],
                prologue: None,
                readonly: true,
            },
            vocab,
            &mut taken,
        );
        methods.extend(code);
        skipped.extend(sk);
        current = deref_target(files, &t);
    }
    let reached = skipped
        .iter()
        .filter_map(|s| {
            let t = s.split("(returns ").nth(1)?.trim_end_matches(')');
            match vocab.classify(t) {
                Some(Js::Handle(x)) | Some(Js::HandleList(x)) => Some(x),
                _ => None,
            }
        })
        .collect();
    if methods.is_empty() {
        return (TokenStream::new(), skipped, reached);
    }
    // A def that owns a content group gets a node view over it, derived from the
    // presence of `root() -> &Group`.
    let owns_group = files.iter().flat_map(|f| &f.items).any(|i| {
        let Item::Impl(imp) = i else { return false };
        imp.trait_.is_none()
            && ty_str(&imp.self_ty).rsplit("::").next()
                == Some(ty.rsplit("::").next().unwrap_or(ty))
            && imp.items.iter().any(|it| match it {
                ImplItem::Fn(f) => {
                    is_pub(&f.vis)
                        && f.sig.ident == "root"
                        && matches!(&f.sig.output, ReturnType::Type(_, t) if ty_str(t) == "&Group")
                }
                _ => false,
            })
    });
    let content = if owns_group {
        quote! {
            impl HasRoot for #path {
                fn group(&self) -> &usvg::Group {
                    self.root()
                }
            }

            #[napi]
            impl #name {
                #[doc = " Children of this definition's content group."]
                #[napi]
                pub fn children(&self) -> Vec<SvgNode> {
                    let base = NodeBase::Def(self.inner.clone());
                    (0..self.inner.root().children().len())
                        .map(|i| SvgNode { tree: None, base: base.clone(), path: vec![i as u32] })
                        .collect()
                }
            }
        }
    } else {
        TokenStream::new()
    };
    let doc = format!(" Read-only handle on a `usvg::{ty}`.");
    let code = quote! {
        #[doc = #doc]
        #[napi]
        pub struct #name {
            inner: std::sync::Arc<#path>,
        }

        impl #name {
            fn wrap(inner: std::sync::Arc<#path>) -> Self {
                Self { inner }
            }
        }

        #[napi]
        impl #name {
            #methods
        }

        #content
    };
    (code, skipped, reached)
}

/// What one mapping pass reads: which sources, which upstream type, how the
/// wrapper reaches its receiver, and what to leave alone. A struct because the
/// `Pass` table in `main` already carries these, and unpacking it into
/// positional arguments was how this grew to eight of them.
struct MethodPass<'a> {
    files: &'a [syn::File],
    ty: &'a str,
    receiver: &'a TokenStream,
    skip: &'a [&'a str],
    /// Emitted before the call; when set, every wrapper returns `Result<_>` so a
    /// receiver that has to be looked up first can fail cleanly.
    prologue: Option<&'a TokenStream>,
    /// An `Arc` receiver cannot hand out `&mut`, so drop mutating methods.
    readonly: bool,
}

fn map_methods(
    m: &MethodPass,
    vocab: &Vocab,
    // Method names already emitted on the target class: two passes can land on
    // the same `impl` block (`Tree::filters` and `Group::filters` both do).
    taken: &mut BTreeSet<String>,
) -> (TokenStream, Vec<String>) {
    // Destructured by name so the body below is unchanged from when these were
    // eight parameters.
    let MethodPass {
        files,
        ty,
        receiver,
        skip,
        prologue,
        readonly,
    } = *m;
    let mut code = TokenStream::new();
    let mut skipped = Vec::new();

    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || ty_str(&imp.self_ty).rsplit("::").next() != Some(ty) {
            continue;
        }

        for it in &imp.items {
            let ImplItem::Fn(f) = it else { continue };
            if !is_pub(&f.vis) {
                continue;
            }
            let name = f.sig.ident.to_string();
            if skip.contains(&name.as_str()) {
                continue; // renderer internals, no meaning on the JS side
            }
            if !taken.insert(name.clone()) {
                continue; // cfg-gated twin, or already emitted by another pass
            }

            // generic param -> bound text, so `P: AsRef<Path>` can be classified
            let bounds: BTreeMap<String, String> = f
                .sig
                .generics
                .type_params()
                .map(|p| (p.ident.to_string(), quote!(#p).to_string().replace(' ', "")))
                .collect();

            let mut inputs = f.sig.inputs.iter();
            let Some(syn::FnArg::Receiver(recv)) = inputs.next() else {
                skipped.push(format!("{name} (no receiver)"));
                continue;
            };
            // syn 3 moved the `&mut self` marker into `ReceiverKind::Reference`;
            // `Receiver::mutability` now only covers by-value `mut self`.
            let mutable = match &recv.kind {
                syn::ReceiverKind::Reference(_, _, m) => m.is_some(),
                _ => {
                    skipped.push(format!("{name} (consumes self)"));
                    continue;
                }
            };

            let mut args = Vec::new();
            let mut ok = true;
            for a in inputs {
                let syn::FnArg::Typed(t) = a else { continue };
                let ty = ty_str(&t.ty);
                let bound = bounds.get(&ty).map(String::as_str).unwrap_or("");
                // A generic parameter is classified through its bound instead.
                let kind = if bound.contains("AsRef<std::path::Path>")
                    || bound.contains("AsRef<Path>")
                    || ty == "&std::path::Path"
                    || ty == "std::path::PathBuf"
                {
                    Some(Arg::PathStr)
                } else if bound.contains("Into<String>") || bound.contains("AsRef<str>") {
                    Some(Arg::Str)
                } else {
                    match vocab.classify(&ty) {
                        Some(Js::Bytes) => Some(Arg::Bytes),
                        Some(Js::Str) => Some(Arg::Str),
                        _ if ty == "ID" => Some(Arg::Face),
                        _ => None,
                    }
                };
                match kind {
                    Some(k) => args.push((t.pat.clone(), k)),
                    None => {
                        ok = false;
                        skipped.push(format!("{name} (arg: {ty})"));
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }

            let ret = match &f.sig.output {
                ReturnType::Default => Ret::Unit,
                ReturnType::Type(_, t) => {
                    let ty = ty_str(t);
                    match vocab.classify(&ty) {
                        Some(Js::Bool) => Ret::Bool,
                        Some(Js::Count) => Ret::Count,
                        Some(Js::Str) => Ret::Text,
                        Some(Js::Bbox) => Ret::Box_,
                        Some(Js::OptBbox) => Ret::OptBox,
                        Some(Js::TryUnit) => Ret::TryUnit,
                        Some(Js::F32) | Some(Js::F64) => Ret::Num,
                        Some(Js::U32) | Some(Js::U8) => Ret::Int,
                        Some(Js::Matrix) => Ret::Matrix,
                        Some(Js::Scalar) => Ret::Scalar,
                        Some(Js::Size) => Ret::Dims,
                        Some(Js::Enum) => Ret::Enum(format_ident!("{}", vocab.resolve(&ty))),
                        Some(Js::Handle(t)) => Ret::Handle(t),
                        Some(Js::HandleList(t)) => Ret::HandleList(t),
                        Some(Js::Object(t)) => Ret::Object(t),
                        Some(Js::ObjectList(t)) => Ret::ObjectList(t),
                        Some(Js::Value(t)) => Ret::Value(t),
                        Some(Js::ValueList(t)) => Ret::ValueList(t),
                        Some(Js::OptObject(t)) => Ret::OptObject(t),
                        Some(Js::OptValue(t)) => Ret::OptValue(t),
                        Some(Js::IntNewtype(_)) => Ret::IntNewtype,
                        _ => {
                            skipped.push(format!("{name} (returns {ty})"));
                            continue;
                        }
                    }
                }
            };

            let ident = &f.sig.ident;
            let params: Vec<TokenStream> = args
                .iter()
                .map(|(pat, kind)| match kind {
                    Arg::Bytes => quote!(#pat: Buffer),
                    Arg::Face => quote!(#pat: &FontFace),
                    _ => quote!(#pat: String),
                })
                .collect();
            let fwd: Vec<TokenStream> = args
                .iter()
                .map(|(pat, kind)| match kind {
                    Arg::Bytes => quote!(#pat.to_vec()),
                    Arg::Face => quote!(#pat.inner.id),
                    _ => quote!(#pat),
                })
                .collect();
            if readonly && mutable {
                skipped.push(format!("{name} (needs &mut self)"));
                continue;
            }
            let recv = if mutable {
                quote!(&mut self)
            } else {
                quote!(&self)
            };
            let doc = docs(&f.attrs);
            let call = quote!(#receiver.#ident(#(#fwd),*));

            // One place decides the JS type and the expression producing it.
            let (ret_ty, value, already_result) = match ret {
                Ret::Unit => (quote!(()), quote!(#call), false),
                Ret::Bool => (quote!(bool), quote!(#call), false),
                Ret::Count => (quote!(u32), quote!(#call as u32), false),
                Ret::Text => (quote!(String), quote!(#call.to_string()), false),
                Ret::Box_ => (quote!(BBox), quote!(BBox::from(#call)), false),
                Ret::OptBox => (quote!(Option<BBox>), quote!(#call.map(BBox::from)), false),
                Ret::Num => (quote!(f64), quote!(#call as f64), false),
                Ret::IntNewtype => (quote!(u32), quote!(#call.0 as u32), false),
                Ret::Object(ref t) => {
                    let o = format_ident!("{}", t);
                    (quote!(#o), quote!(#o::from(#call)), false)
                }
                Ret::ObjectList(ref t) => {
                    let o = format_ident!("{}", t);
                    (
                        quote!(Vec<#o>),
                        quote!(#call.into_iter().map(#o::from).collect()),
                        false,
                    )
                }
                Ret::OptObject(ref t) => {
                    let o = format_ident!("{}", t);
                    (
                        quote!(Option<#o>),
                        quote!(#call.map(|x| #o::from(x))),
                        false,
                    )
                }
                Ret::OptValue(ref t) => {
                    let w = value_class_ident(t);
                    (
                        quote!(Option<#w>),
                        quote!(#call.map(|x| #w::wrap(x.clone()))),
                        false,
                    )
                }
                Ret::Value(ref t) => {
                    let w = value_class_ident(t);
                    (quote!(#w), quote!(#w::wrap(#call.clone())), false)
                }
                Ret::ValueList(ref t) => {
                    let w = value_class_ident(t);
                    (
                        quote!(Vec<#w>),
                        // `into_iter` covers both a slice of values and an
                        // iterator of references.
                        quote!(#call.into_iter().map(|x| #w::wrap(x.clone())).collect()),
                        false,
                    )
                }
                Ret::Int => (quote!(u32), quote!(#call as u32), false),
                Ret::Matrix => (quote!(Matrix), quote!(Matrix::from(#call)), false),
                Ret::Scalar => (quote!(f64), quote!(#call.get() as f64), false),
                Ret::Dims => (quote!(Dimensions), quote!(Dimensions::from(#call)), false),
                Ret::Enum(ref e) => (quote!(#e), quote!(#e::from(#call)), false),
                Ret::Handle(ref t) => {
                    let w = wrapper_ident(t);
                    (quote!(#w), quote!(#w::wrap(#call.clone())), false)
                }
                Ret::HandleList(ref t) => {
                    let w = wrapper_ident(t);
                    (
                        quote!(Vec<#w>),
                        quote!(#call.iter().cloned().map(#w::wrap).collect()),
                        false,
                    )
                }
                Ret::TryUnit => (
                    quote!(()),
                    quote!(#call.map_err(|e| Error::from_reason(e.to_string()))?),
                    true,
                ),
            };

            code.extend(if already_result || prologue.is_some() {
                quote! {
                    #(#doc)*
                    #[napi]
                    pub fn #ident(#recv, #(#params),*) -> Result<#ret_ty> {
                        #prologue
                        Ok(#value)
                    }
                }
            } else {
                quote! {
                    #(#doc)*
                    #[napi]
                    pub fn #ident(#recv, #(#params),*) -> #ret_ty { #value }
                }
            });
        }
    }

    (code, skipped)
}

// ---------------------------------------------------------------------------
// 2. signature guards: fail the build loudly if upstream changed shape
// ---------------------------------------------------------------------------

/// One async twin to generate: a Send-safe core the template marked with
/// `#[async_twin(js_method, JsType)]`.
struct Twin {
    target: syn::Type,
    core: syn::Ident,
    /// JS name of the sync method that calls this core, for the doc comment.
    sibling: Option<String>,
    public: syn::Ident,
    js: syn::Ident,
    params: Vec<(syn::Ident, syn::Type)>,
    output: syn::Type,
}

/// Collects the marked cores and *removes* the marker, which is not a real
/// attribute: rustc would reject it. Reading the signature is enough to write
/// the twin, so nothing about it is declared twice.
fn async_twins(file: &mut syn::File) -> Vec<Twin> {
    let mut twins = Vec::new();
    for item in &mut file.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() {
            continue;
        }
        // The sync sibling of a core is the `#[napi]` method whose body calls it:
        // `render_png` calls `png_bytes`. Found here so the generated doc can
        // name what a reader recognises, instead of the private core.
        let siblings: Vec<(String, String)> = imp
            .items
            .iter()
            .filter_map(|it| {
                let ImplItem::Fn(f) = it else { return None };
                let body = quote!(#f).to_string();
                let js = f
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("napi"))
                    .and_then(|a| {
                        let text = quote!(#a).to_string();
                        text.split("js_name = \"")
                            .nth(1)
                            .and_then(|r| r.split('"').next())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| lower_camel(&f.sig.ident.to_string()));
                Some((body, js))
            })
            .collect();
        for it in &mut imp.items {
            let ImplItem::Fn(f) = it else { continue };
            let Some(pos) = f.attrs.iter().position(|a| a.path().is_ident("async_twin")) else {
                continue;
            };
            let attr = f.attrs.remove(pos);
            let args: Vec<syn::Ident> = attr
                .parse_args_with(|input: &syn::parse::ParseBuffer| {
                    syn::punctuated::Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated(
                        input,
                    )
                })
                .unwrap_or_else(|e| panic!("#[async_twin(js_method, JsType)]: {e}"))
                .into_iter()
                .collect();
            let [public, js] = args.as_slice() else {
                panic!("#[async_twin] takes the JS method name and its JS type")
            };
            let params = f
                .sig
                .inputs
                .iter()
                .filter_map(|a| match a {
                    syn::FnArg::Typed(t) => match &*t.pat {
                        syn::Pat::Ident(p) => Some((p.ident.clone(), (*t.ty).clone())),
                        _ => panic!("async twin cores take plain named arguments"),
                    },
                    syn::FnArg::Receiver(_) => None,
                })
                .collect();
            let ReturnType::Type(_, ret) = &f.sig.output else {
                panic!("an async twin core returns Result<T>")
            };
            let output =
                result_inner(ret).unwrap_or_else(|| panic!("{}: expected Result<T>", f.sig.ident));
            let needle = format!("self . {} (", f.sig.ident);
            let sibling = siblings
                .iter()
                .find(|(body, _)| body.contains(&needle))
                .map(|(_, js)| js.clone());
            twins.push(Twin {
                target: (*imp.self_ty).clone(),
                core: f.sig.ident.clone(),
                sibling,
                public: public.clone(),
                js: js.clone(),
                params,
                output,
            });
        }
    }
    twins
}

/// `Result<T>` -> `T`.
fn result_inner(ty: &Type) -> Option<Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    })
}

/// The ceremony: a `Task` type per core, the public `#[napi]` method that
/// schedules it, and the `AbortSignal` plumbing. Written once, applied by rule.
fn emit_twins(twins: &[Twin]) -> TokenStream {
    let mut out = TokenStream::new();
    for t in twins {
        // `Resvg` + `png_bytes` -> `ResvgPngBytesTask`, keeping the target's own
        // casing: lowercasing it first produced `SvgnodePngBytesTask`.
        let task = format_ident!(
            "{}{}Task",
            ty_str(&t.target),
            heck_camel(&t.core.to_string())
        );
        let (names, types): (Vec<_>, Vec<_>) = t.params.iter().cloned().unzip();
        let core = &t.core;
        let public = &t.public;
        let target = &t.target;
        let output = &t.output;
        let js = &t.js;
        // `string` in TypeScript, but `String` in Rust.
        let ts = if js == "String" {
            "string".to_string()
        } else {
            js.to_string()
        };
        let ts_return = format!("Promise<{ts}>");
        let doc = match &t.sibling {
            Some(js) => format!(
                " `{js}` on a worker thread: the work leaves the event loop, and a\n queued call is dropped when the signal fires."
            ),
            None => " Runs on a worker thread; a queued call is dropped when the signal fires."
                .to_string(),
        };
        out.extend(quote! {
            #[doc = #doc]
            pub struct #task {
                recv: #target,
                #(#names: #types,)*
            }

            impl Task for #task {
                type Output = #output;
                type JsValue = <#output as IntoJs>::Js;

                fn compute(&mut self) -> Result<Self::Output> {
                    self.recv.#core(#(self.#names.clone()),*)
                }

                fn resolve(&mut self, _: napi::Env, out: Self::Output) -> Result<Self::JsValue> {
                    Ok(out.into_js())
                }
            }

            #[napi]
            impl #target {
                #[doc = #doc]
                #[napi(ts_return_type = #ts_return)]
                pub fn #public(
                    &self,
                    #(#names: #types,)*
                    signal: Option<AbortSignal>,
                ) -> AsyncTask<#task> {
                    AsyncTask::with_optional_signal(
                        #task { recv: self.clone(), #(#names,)* },
                        signal,
                    )
                }
            }
        });
    }
    out
}

/// `render_png` -> `renderPng`, the name napi gives it in JS.
fn lower_camel(s: &str) -> String {
    let camel = heck_camel(s);
    let mut c = camel.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `resvg_png_bytes` -> `ResvgPngBytes`.
fn heck_camel(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Method names the emitter template defines itself, per target type, read off
/// the template rather than listed by hand: `impl Resvg { pub fn children }`
/// means the `children` upstream method is covered, and a rule can say so.
fn template_fns(code: &TokenStream) -> BTreeMap<String, Vec<String>> {
    let file: syn::File =
        syn::parse2(code.clone()).expect("the emitter template is not valid Rust on its own");
    // A Vec, not a Set: the duplicate check below needs the repeats.
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in &file.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() {
            continue; // trait impls are plumbing, not API
        }
        let target = ty_str(&imp.self_ty);
        let names = out.entry(target).or_default();
        for it in &imp.items {
            if let ImplItem::Fn(f) = it {
                names.push(f.sig.ident.to_string());
            }
        }
    }
    out
}

/// Upstream names the template exposes under a *different* one. A rule cannot
/// find these -- the rename is the decision -- so they are declared, and
/// checked: a stale entry fails the build instead of rotting in a log line.
const RENAMED: &[(&str, &str)] = &[
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

fn assert_free_fn(files: &[syn::File], name: &str, want: &[&str]) {
    let f = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Fn(f) if f.sig.ident == name && is_pub(&f.vis) => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("resvg: `pub fn {name}` disappeared"));
    let got: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|a| match a {
            syn::FnArg::Typed(t) => ty_str(&t.ty),
            syn::FnArg::Receiver(_) => "self".into(),
        })
        .collect();
    assert_eq!(
        got,
        want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "resvg::{name} signature changed; update build.rs"
    );
}

/// The emitter templates code against these fields by name; break loudly if they move.
fn assert_struct_fields(files: &[syn::File], ty: &str, want: &[&str]) {
    let s = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("usvg: `pub struct {ty}` not found"));
    let Fields::Named(named) = &s.fields else {
        panic!("usvg::{ty} is no longer a struct with named fields")
    };
    for w in want {
        assert!(
            named
                .named
                .iter()
                .any(|f| f.ident.as_ref().is_some_and(|i| i == w)),
            "usvg::{ty}::{w} disappeared; update build.rs"
        );
    }
}

fn assert_tree_ctor(files: &[syn::File], name: &str) {
    let found = files.iter().flat_map(|f| &f.items).any(|i| match i {
        // upstream writes `impl crate::Tree`, so compare the last path segment
        Item::Impl(imp) if ty_str(&imp.self_ty).rsplit("::").next() == Some("Tree") => imp
            .items
            .iter()
            .any(|it| matches!(it, ImplItem::Fn(f) if f.sig.ident == name && is_pub(&f.vis))),
        _ => false,
    });
    assert!(found, "usvg: `Tree::{name}` disappeared");
}

// ---------------------------------------------------------------------------
// 4. emission
// ---------------------------------------------------------------------------

/// The fragments the passes produce. Grouped so `template` keeps a short
/// signature: these are the same on both of its calls, while the four method
/// lists are not.
struct Fragments<'a> {
    enums: TokenStream,
    object_code: TokenStream,
    wrapper_code: TokenStream,
    decls: Vec<&'a TokenStream>,
    assigns: Vec<&'a TokenStream>,
    write_decls: Vec<&'a TokenStream>,
    write_assigns: Vec<&'a TokenStream>,
}

/// The hand-written half of the binding: everything the passes cannot derive.
///
/// Called twice. Once with empty method lists, so `template_fns` can read the
/// method names it defines off the template itself -- nothing lists them by
/// hand -- and once for real.
///
/// NOTE: no inner attributes (`#![..]` / `//!`) in the body: the file is pulled
/// in with `include!`, which only accepts items.
fn template(
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
        fn href_resolver(images: ImageMap, misses: Misses) -> usvg::ImageHrefResolver<'static> {
            let from_disk = usvg::ImageHrefResolver::default_string_resolver();
            let sniff = usvg::ImageHrefResolver::default_data_resolver();
            usvg::ImageHrefResolver {
                resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
                resolve_string: Box::new(move |href: &str, opts: &usvg::Options| {
                    // 1. buffer supplied by JS -- let usvg sniff the format itself
                    if let Some(data) = images.get(href) {
                        return sniff("text/plain", data.clone(), opts);
                    }
                    // 2. usvg default: path relative to `resourcesDir`
                    if let Some(kind) = from_disk(href, opts) {
                        return Some(kind);
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

        #[doc = " Opaque wrapper over `fontdb::Database` (memory-mapped faces, no JSON form)."]
        #[napi]
        pub struct FontDatabase {
            inner: usvg::fontdb::Database,
        }

        impl FontDatabase {
            #[doc = " Snapshot of a shared database, e.g. the one a parse resolved."]
            fn wrap(inner: std::sync::Arc<usvg::fontdb::Database>) -> Self {
                Self { inner: (*inner).clone() }
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
                let names: Vec<usvg::fontdb::Family> = families
                    .iter()
                    .map(|f| match f.as_str() {
                        "serif" => usvg::fontdb::Family::Serif,
                        "sans-serif" => usvg::fontdb::Family::SansSerif,
                        "cursive" => usvg::fontdb::Family::Cursive,
                        "fantasy" => usvg::fontdb::Family::Fantasy,
                        "monospace" => usvg::fontdb::Family::Monospace,
                        other => usvg::fontdb::Family::Name(other),
                    })
                    .collect();
                let query = usvg::fontdb::Query {
                    families: &names,
                    weight: usvg::fontdb::Weight(weight.unwrap_or(400) as u16),
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

            #[napi(constructor)]
            pub fn new() -> Self {
                Self { inner: usvg::fontdb::Database::new() }
            }
            #fontdb_methods
        }

        #[doc = " Mirror of `usvg::WriteOptions`, for `Resvg.toString()`. Every field is"]
        #[doc = " optional; omitted fields keep the usvg default."]
        #[napi(object)]
        #[derive(Default, Clone)]
        pub struct WriteOptions {
            #(#write_decls)*
        }

        impl WriteOptions {
            fn to_usvg(&self) -> usvg::WriteOptions {
                let mut o = usvg::WriteOptions::default();
                #(#write_assigns)*
                o
            }
        }

        #[doc = " Output size / scaling of one render pass."]
        #[napi(object)]
        #[derive(Default, Clone)]
        pub struct RenderParams {
            #[doc = " Uniform scale factor. Default: 1."]
            pub scale: Option<f64>,
            #[doc = " Target width in px; keeps the aspect ratio and overrides `scale`."]
            pub width: Option<u32>,
            #[doc = " Target height in px; keeps the aspect ratio and overrides `scale`."]
            pub height: Option<u32>,
            #[doc = " Background colour: any CSS3 colour string, e.g. `#eee`, `teal`,"]
            #[doc = " `rgba(255, 0, 0, .5)`. Default: transparent."]
            pub background: Option<String>,
            #[doc = " Render only this rectangle, in SVG user units. `scale` / `width` /"]
            #[doc = " `height` then size the crop, not the full viewport. Feed it"]
            #[doc = " `absLayerBoundingBox()` to trim the document to its content."]
            pub crop: Option<BBox>,
        }

        #[doc = " An affine transform, same field order as SVG's `matrix(...)`."]
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
                    sx: t.sx as f64, kx: t.kx as f64, ky: t.ky as f64,
                    sy: t.sy as f64, tx: t.tx as f64, ty: t.ty as f64,
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
                Self { width: s.width() as f64, height: s.height() as f64 }
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
                    x: r.x() as f64,
                    y: r.y() as f64,
                    width: r.width() as f64,
                    height: r.height() as f64,
                }
            }
        }

        impl From<usvg::NonZeroRect> for BBox {
            fn from(r: usvg::NonZeroRect) -> Self {
                Self {
                    x: r.x() as f64,
                    y: r.y() as f64,
                    width: r.width() as f64,
                    height: r.height() as f64,
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

        struct LogCollector;
        static LOG_COLLECTOR: LogCollector = LogCollector;

        impl log::Log for LogCollector {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }

            fn log(&self, record: &log::Record) {
                if let Ok(mut buf) = LOGS.lock() {
                    // Bounded: a pathological document must not grow this forever.
                    if buf.len() < 500 {
                        buf.push(format!(
                            "{} {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        ));
                    }
                }
            }

            fn flush(&self) {}
        }

        #[doc = " Starts collecting what usvg and resvg report."]
        #[doc = ""]
        #[doc = " `level` is `off`, `error`, `warn`, `info`, `debug` or `trace`. Safe to"]
        #[doc = " call repeatedly: the logger is installed once, the level always applies."]
        #[napi]
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

        #[doc = " Drains the messages collected since the last call."]
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

        fn node_at<'t>(root: &'t usvg::Group, path: &[u32]) -> Option<&'t usvg::Node> {
            let (last, rest) = path.split_last()?;
            let mut group = root;
            for i in rest {
                match group.children().get(*i as usize)? {
                    usvg::Node::Group(g) => group = g,
                    _ => return None,
                }
            }
            group.children().get(*last as usize)
        }

        #[doc = " Depth-first search for an element id, returning its index path."]
        fn path_of_id(group: &usvg::Group, id: &str, prefix: &mut Vec<u32>) -> Option<Vec<u32>> {
            for (i, child) in group.children().iter().enumerate() {
                prefix.push(i as u32);
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

        #[doc = " `Paint::Color`: a colour resolved by the parser."]
        #[napi(object)]
        pub struct ColorPaint {
            #[doc = " Discriminant. Narrow on this."]
            #[napi(ts_type = "'color'")]
            pub r#type: String,
            pub color: Color,
        }

        #[doc = " A paint server -- gradient or pattern -- named by id, the way the"]
        #[doc = " document itself refers to one with `url(#id)`. Resolve it through"]
        #[doc = " `linearGradients()`, `radialGradients()` or `patterns()`."]
        #[doc = ""]
        #[doc = " An id rather than the object: a paint server is shared by every"]
        #[doc = " element that references it, so handing out a copy per element would"]
        #[doc = " misrepresent the document."]
        #[napi(object)]
        pub struct PaintServer {
            #[doc = " Discriminant. Narrow on this."]
            #[napi(ts_type = "'linearGradient' | 'radialGradient' | 'pattern'")]
            pub r#type: String,
            pub id: String,
        }

        #[doc = " `usvg::Paint` is an enum carrying a payload, which napi cannot map on"]
        #[doc = " its own -- hence the hand-written split into a discriminated union."]
        fn paint_js(p: &usvg::Paint) -> Either<ColorPaint, PaintServer> {
            let server = |kind: &str, id: &str| {
                Either::B(PaintServer { r#type: kind.to_string(), id: id.to_string() })
            };
            match p {
                usvg::Paint::Color(c) => Either::A(ColorPaint {
                    r#type: "color".to_string(),
                    color: Color::from(c),
                }),
                // id() reaches BaseGradient through Deref for both gradients.
                usvg::Paint::LinearGradient(g) => server("linearGradient", g.id()),
                usvg::Paint::RadialGradient(g) => server("radialGradient", g.id()),
                usvg::Paint::Pattern(p) => server("pattern", p.id()),
            }
        }

        #[doc = " A read-only handle on one element of the parsed tree."]
        #[doc = ""]
        #[doc = " usvg stores nodes as `Box`, not `Arc`, so a handle cannot own one. It"]
        #[doc = " keeps the tree alive plus the child-index path instead, and re-resolves"]
        #[doc = " on each call -- safe because a parsed tree is immutable."]
        #[napi]
        #[derive(Clone)]
        pub struct SvgNode {
            #[doc = " The document, when there is one: a node reached through a def has"]
            #[doc = " no document context, so the def tables are out of reach from it."]
            tree: Option<std::sync::Arc<usvg::Tree>>,
            base: NodeBase,
            path: Vec<u32>,
        }

        impl SvgNode {
            fn node(&self) -> Result<&usvg::Node> {
                node_at(self.base.group(), &self.path)
                    .ok_or_else(|| Error::from_reason("node path no longer resolves"))
            }

            fn child(&self, i: usize) -> Self {
                let mut path = self.path.clone();
                path.push(i as u32);
                Self { tree: self.tree.clone(), base: self.base.clone(), path }
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

            #[doc = " Fill paint of a shape, or null: for a node that is not a path, and"]
            #[doc = " for a path the document leaves unfilled."]
            #[doc = ""]
            #[doc = " Reached from the node rather than through a `Path` class: the paint"]
            #[doc = " is the useful half, and it needs no class to hand it over."]
            #[napi]
            pub fn fill_paint(&self) -> Result<Option<Either<ColorPaint, PaintServer>>> {
                Ok(match self.node()? {
                    usvg::Node::Path(p) => p.fill().map(|f| paint_js(f.paint())),
                    _ => None,
                })
            }

            #[doc = " Stroke paint of a shape, or null. Same shape as `fillPaint`."]
            #[napi]
            pub fn stroke_paint(&self) -> Result<Option<Either<ColorPaint, PaintServer>>> {
                Ok(match self.node()? {
                    usvg::Node::Path(p) => p.stroke().map(|s| paint_js(s.paint())),
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
            #[doc = " so it is matched by `id` against the document's clip-path table."]
            #[doc = " Returns `null` for an unnamed clip path."]
            #[napi]
            pub fn clip_path(&self) -> Result<Option<ClipPath>> {
                let usvg::Node::Group(g) = self.node()? else {
                    return Ok(None);
                };
                let Some(target) = g.clip_path() else {
                    return Ok(None);
                };
                let Some(tree) = &self.tree else {
                    return Ok(None); // reached through a def: no document context
                };
                Ok(tree
                    .clip_paths()
                    .iter()
                    .find(|c| !target.id().is_empty() && c.id() == target.id())
                    .cloned()
                    .map(ClipPath::wrap))
            }

            #[doc = " Mask applied to this element, matched by `id` like `clipPath`."]
            #[napi]
            pub fn mask(&self) -> Result<Option<Mask>> {
                let usvg::Node::Group(g) = self.node()? else {
                    return Ok(None);
                };
                let Some(target) = g.mask() else {
                    return Ok(None);
                };
                let Some(tree) = &self.tree else {
                    return Ok(None); // reached through a def: no document context
                };
                Ok(tree
                    .masks()
                    .iter()
                    .find(|m| !target.id().is_empty() && m.id() == target.id())
                    .cloned()
                    .map(Mask::wrap))
            }

            #[doc = " The Send half: bytes, no JS handle, so a worker thread can run it."]
            #[async_twin(render_png_async, Buffer)]
            fn png_bytes(&self, params: Option<RenderParams>) -> Result<Vec<u8>> {
                render_node_png(self.node()?, &params.unwrap_or_default())
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
        #[doc = " `Clone` because an async twin captures the receiver: every field is"]
        #[doc = " behind an `Arc` or cheap to copy, so the clone is a refcount bump and"]
        #[doc = " the worker thread never touches the JS heap."]
        #[derive(Clone)]
        pub struct Resvg {
            tree: std::sync::Arc<usvg::Tree>,
            #[doc = " Source kept verbatim: resolving an image means re-parsing."]
            svg: std::sync::Arc<Vec<u8>>,
            options: RenderOptions,
            fonts: std::sync::Arc<usvg::fontdb::Database>,
            images: ImageMap,
            pending_images: Vec<String>,
            pending_fonts: Vec<String>,
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
                let svg = match svg {
                    Either::A(text) => text.into_bytes(),
                    Either::B(data) => data.to_vec(),
                };
                let options = options.unwrap_or_default();
                let fonts = match fonts {
                    Some(f) => std::sync::Arc::new(f.inner.clone()),
                    None => default_fontdb(),
                };
                let images: ImageMap = images
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(href, buf)| (href, std::sync::Arc::new(buf.to_vec())))
                    .collect();
                let (tree, pending_images, pending_fonts) =
                    Self::build(&svg, &options, fonts.clone(), &images)?;
                Ok(Self {
                    tree: std::sync::Arc::new(tree),
                    svg: std::sync::Arc::new(svg),
                    options,
                    fonts,
                    images,
                    pending_images,
                    pending_fonts,
                })
            }

            #[doc = " Hrefs of `<image>` elements that neither the supplied buffers nor the"]
            #[doc = " filesystem could resolve. Feed them back with `resolveImage`."]
            #[napi]
            pub fn pending_images(&self) -> Vec<String> {
                self.pending_images.clone()
            }

            #[doc = " Named font families requested by `<text>` that the database does not"]
            #[doc = " carry, so the text was rendered with a fallback face. Load them with"]
            #[doc = " `FontDatabase` and re-parse to fix the output."]
            #[napi]
            pub fn pending_fonts(&self) -> Vec<String> {
                self.pending_fonts.clone()
            }

            #[doc = " Supplies one image and re-parses the document."]
            #[doc = ""]
            #[doc = " Pass everything you already have through the constructor's `images`"]
            #[doc = " argument instead: this re-parses once per call."]
            #[napi]
            pub fn resolve_image(&mut self, href: String, data: Buffer) -> Result<()> {
                self.images.insert(href, std::sync::Arc::new(data.to_vec()));
                let (tree, pending_images, pending_fonts) =
                    Self::build(&self.svg, &self.options, self.fonts.clone(), &self.images)?;
                self.tree = std::sync::Arc::new(tree);
                self.pending_images = pending_images;
                self.pending_fonts = pending_fonts;
                Ok(())
            }

            #[napi(getter)]
            pub fn width(&self) -> f64 {
                self.tree.size().width() as f64
            }

            #[napi(getter)]
            pub fn height(&self) -> f64 {
                self.tree.size().height() as f64
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
                signal: Option<AbortSignal>,
            ) -> AsyncTask<ParseTask> {
                AsyncTask::with_optional_signal(
                    ParseTask {
                        svg: match svg {
                            Either::A(text) => text.into_bytes(),
                            Either::B(data) => data.to_vec(),
                        },
                        options: options.unwrap_or_default(),
                        fonts: match fonts {
                            Some(f) => std::sync::Arc::new(f.inner.clone()),
                            None => default_fontdb(),
                        },
                        images: images
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(href, buf)| (href, std::sync::Arc::new(buf.to_vec())))
                            .collect(),
                    },
                    signal,
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
                render_node_png(node, &params.unwrap_or_default())
            }

            #[doc = " Handle on one element, by `id`."]
            #[napi]
            pub fn node(&self, id: String) -> Option<SvgNode> {
                path_of_id(self.tree.root(), &id, &mut Vec::new()).map(|path| SvgNode {
                    tree: Some(self.tree.clone()),
                    base: NodeBase::Tree(self.tree.clone()),
                    path,
                })
            }

            #[doc = " Direct children of the root group."]
            #[napi]
            pub fn children(&self) -> Vec<SvgNode> {
                (0..self.tree.root().children().len())
                    .map(|i| SvgNode {
                        tree: Some(self.tree.clone()),
                        base: NodeBase::Tree(self.tree.clone()),
                        path: vec![i as u32],
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
                let opt = options.unwrap_or_default().to_usvg();
                let tree = &self.tree;
                // Numbers come from JS and the writer indexes a few tables
                // unchecked; an unwind reaching the extern "C" frame would abort
                // the process, so stop it here.
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    usvg::Tree::to_string(tree, &opt)
                }))
                .map_err(|_| Error::from_reason("usvg panicked while writing the SVG"))
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
            fn build(
                svg: &[u8],
                options: &RenderOptions,
                fonts: std::sync::Arc<usvg::fontdb::Database>,
                images: &ImageMap,
            ) -> Result<(usvg::Tree, Vec<String>, Vec<String>)> {
                let missing_images = Misses::default();
                let missing_fonts = Misses::default();
                let mut opts = options.to_usvg(fonts);
                opts.image_href_resolver =
                    href_resolver(images.clone(), missing_images.clone());
                opts.font_resolver = font_resolver(missing_fonts.clone());
                let tree = usvg::Tree::from_data(svg, &opts)
                    .map_err(|e| Error::from_reason(format!("invalid SVG: {e}")))?;
                Ok((
                    tree,
                    std::mem::take(&mut *missing_images.0.lock().unwrap_or_else(|e| e.into_inner())),
                    std::mem::take(&mut *missing_fonts.0.lock().unwrap_or_else(|e| e.into_inner())),
                ))
            }

            fn draw(&self, p: RenderParams) -> Result<tiny_skia::Pixmap> {
                draw(&self.tree, &p)
            }
        }

        fn draw(tree: &usvg::Tree, p: &RenderParams) -> Result<tiny_skia::Pixmap> {
                let size = tree.size();
                // A crop replaces the viewport: width/height/scale then size the
                // cropped region, which is what you want after a bbox call.
                let (base_w, base_h, off_x, off_y) = match &p.crop {
                    Some(c) => (c.width as f32, c.height as f32, c.x as f32, c.y as f32),
                    None => (size.width(), size.height(), 0.0, 0.0),
                };
                if !(base_w > 0.0 && base_h > 0.0) {
                    return Err(Error::from_reason(format!(
                        "empty crop: {base_w}x{base_h}"
                    )));
                }
                let scale = if let Some(w) = p.width {
                    w as f32 / base_w
                } else if let Some(h) = p.height {
                    h as f32 / base_h
                } else {
                    p.scale.unwrap_or(1.0) as f32
                };
                if !(scale.is_finite() && scale > 0.0) {
                    return Err(Error::from_reason(format!("invalid scale: {scale}")));
                }
                let w = (base_w * scale).ceil() as u32;
                let h = (base_h * scale).ceil() as u32;
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
                resvg::render(
                    tree,
                    tiny_skia::Transform::from_scale(scale, scale)
                        .pre_translate(-off_x, -off_y),
                    &mut pixmap.as_mut(),
                );
                Ok(pixmap)
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
                let scale = if let Some(w) = p.width {
                    w as f32 / bbox.width()
                } else if let Some(h) = p.height {
                    h as f32 / bbox.height()
                } else {
                    p.scale.unwrap_or(1.0) as f32
                };
                if !(scale.is_finite() && scale > 0.0) {
                    return Err(Error::from_reason(format!("invalid scale: {scale}")));
                }
                let w = (bbox.width() * scale).ceil() as u32;
                let h = (bbox.height() * scale).ceil() as u32;
                let mut pixmap = tiny_skia::Pixmap::new(w, h)
                    .ok_or_else(|| Error::from_reason(format!("bad pixmap size {w}x{h}")))?;
                if let Some(css) = &p.background {
                    let c: svgtypes::Color = css
                        .parse()
                        .map_err(|_| Error::from_reason(format!("invalid background: {css}")))?;
                    pixmap.fill(tiny_skia::Color::from_rgba8(c.red, c.green, c.blue, c.alpha));
                }
                resvg::render_node(
                    node,
                    tiny_skia::Transform::from_scale(scale, scale)
                        .pre_translate(-bbox.x(), -bbox.y())
                        .pre_concat(node.abs_transform())
                        .pre_translate(inner.x(), inner.y()),
                    &mut pixmap.as_mut(),
                )
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
            options: RenderOptions,
            fonts: std::sync::Arc<usvg::fontdb::Database>,
            images: ImageMap,
        }

        impl Task for ParseTask {
            type Output = (usvg::Tree, Vec<String>, Vec<String>);
            type JsValue = Resvg;

            fn compute(&mut self) -> Result<Self::Output> {
                Resvg::build(&self.svg, &self.options, self.fonts.clone(), &self.images)
            }

            fn resolve(&mut self, _: napi::Env, out: Self::Output) -> Result<Self::JsValue> {
                let (tree, pending_images, pending_fonts) = out;
                Ok(Resvg {
                    tree: std::sync::Arc::new(tree),
                    svg: std::sync::Arc::new(std::mem::take(&mut self.svg)),
                    options: self.options.clone(),
                    fonts: self.fonts.clone(),
                    images: std::mem::take(&mut self.images),
                    pending_images,
                    pending_fonts,
                })
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
                    &ImageMap::new(),
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
            signal: Option<AbortSignal>,
        ) -> AsyncTask<RenderTask> {
            AsyncTask::with_optional_signal(
                RenderTask {
                    svg: match svg {
                        Either::A(text) => text.into_bytes(),
                        Either::B(data) => data.to_vec(),
                    },
                    options: options.unwrap_or_default(),
                    params: params.unwrap_or_default(),
                },
                signal,
            )
        }
    }
}

fn main() {
    napi_build::setup();
    println!("cargo::rerun-if-changed=build.rs");
    // The generated file is also an *input* to watch: if someone deletes or
    // stubs src/lib.rs, the script must re-run and regenerate it.
    println!("cargo::rerun-if-changed=src/lib.rs");
    println!("cargo::rerun-if-env-changed=RESVG_NAPI_EMIT_SRC");
    println!("cargo::rerun-if-env-changed=RESVG_NAPI_CODEGEN_LOG");

    let locked = locked_versions();
    let usvg = locate("usvg", "parser/options.rs", &locked);
    let resvg = locate("resvg", "lib.rs", &locked);
    let fontdb = locate("fontdb", "lib.rs", &locked);
    report!("usvg sources: {}", usvg.display());

    // Whole crates, no per-file list: the `impl` blocks we wrap are spread over
    // tree/mod.rs, tree/filter.rs, parser/, writer.rs...
    let usvg_parsed = parse_crate(&usvg);
    let usvg_files: Vec<syn::File> = usvg_parsed.iter().map(|(_, f)| f.clone()).collect();
    let modules = upstream_modules(&usvg_parsed, &public_modules(&usvg_files));
    // Only the crate root for free functions: a module-level `pub fn` elsewhere
    // is not crate-public (resvg has an internal `render(&Image, ...)`).
    let resvg_root = vec![parse(&resvg.join("lib.rs"))];
    let fontdb_files: Vec<syn::File> = parse_crate(&fontdb).into_iter().map(|(_, f)| f).collect();

    // Guards first: no point generating against a moved API.
    assert_free_fn(
        &resvg_root,
        "render",
        &[
            "&usvg::Tree",
            "tiny_skia::Transform",
            "&muttiny_skia::PixmapMut",
        ],
    );
    assert_tree_ctor(&usvg_files, "from_data");
    assert_tree_ctor(&usvg_files, "to_string");
    assert_struct_fields(
        &usvg_files,
        "ImageHrefResolver",
        &["resolve_data", "resolve_string"],
    );
    assert_struct_fields(
        &usvg_files,
        "FontResolver",
        &["select_font", "select_fallback"],
    );

    // POW_VEC has N entries, so the highest usable precision is N - 1.
    let precision_max = (static_array_len(&usvg_files, "POW_VEC") - 1) as u32;
    report!("usvg precision clamp derived from POW_VEC: {precision_max}");

    // Enums to mirror = named by a config field OR by a return of a wrapped impl.
    let mut referenced = struct_field_types(&usvg_files, "Options");
    referenced.extend(struct_field_types(&usvg_files, "WriteOptions"));
    referenced.extend(returned_types(&usvg_files, &[]));
    referenced.extend(returned_types(&fontdb_files, &[]));

    // Object candidates: a *collection* of values wants a value type, whereas a
    // single borrow wants a handle. So `&[Stop]` and `impl Iterator<Item=&FaceInfo>`
    // make Stop/FaceInfo objects, while `&Group` stays a handle question.
    let object_seeds: BTreeSet<String> = referenced
        .iter()
        .filter_map(|t| {
            let b = t.trim_start_matches('&');
            if let Some(inner) = b.strip_prefix("[").and_then(|s| s.strip_suffix("]")) {
                return Some(inner.trim_start_matches('&').to_string());
            }
            if let Some(rest) = b.strip_prefix("implIterator<Item=") {
                return Some(rest.split('>').next()?.trim_start_matches('&').to_string());
            }
            None
        })
        .collect();

    let mut vocab = Vocab::default();
    let extra: Vec<syn::File> = ["tiny-skia-path", "strict-num"]
        .iter()
        .flat_map(|c| parse_crate(&locate(c, "lib.rs", &locked)))
        .map(|(_, f)| f)
        .collect();
    for set in [&usvg_files, &fontdb_files, &extra] {
        vocab.scalars.extend(f32_newtypes(set));
        vocab.aliases.extend(type_aliases(set));
    }
    report!(
        "vocabulary derived: {} f32 newtypes, {} type aliases",
        vocab.scalars.len(),
        vocab.aliases.len()
    );

    let (enum_names, enums) = map_enums(&usvg_files, &referenced, &modules);
    vocab.enums = enum_names.clone();
    for set in [&usvg_files, &fontdb_files] {
        vocab.ints.extend(int_newtypes(set));
    }

    // Fixpoint over object types: a generated object can reference another
    // (`Stop` holds a `Color`).
    let mut object_todo: std::collections::VecDeque<String> = object_seeds.into_iter().collect();
    let mut object_done: BTreeSet<String> = BTreeSet::new();
    let mut object_root: BTreeMap<String, TokenStream> = BTreeMap::new();
    // Phase 1: discover. Generation is only used here to learn what a type
    // reaches; the code is thrown away because the registry is still growing.
    while let Some(t) = object_todo.pop_front() {
        if !object_done.insert(t.clone()) || object_done.len() > 64 {
            continue;
        }
        if vocab.enums.contains(&t) || vocab.scalars.contains(&t) || vocab.ints.contains(&t) {
            continue;
        }
        let (source, root) = if struct_fields_opt(&usvg_files, &t).is_some() {
            (&usvg_files, quote!(usvg))
        } else if struct_fields_opt(&fontdb_files, &t).is_some() {
            (&fontdb_files, quote!(usvg::fontdb))
        } else {
            report!("object {t} skipped: not a public struct");
            continue;
        };
        vocab.objects.insert(t.clone());
        object_root.insert(t.clone(), root.clone());
        let (members, dropped, reached) = data_members(source, &t, &vocab);
        if members.is_empty() {
            vocab.objects.remove(&t);
            object_root.remove(&t);
            report!("data type {t} skipped: nothing mappable on it");
            continue;
        }
        // A dropped member whose type is itself a public struct is a candidate:
        // that is how `Stop` reaches `Color`.
        for d in &dropped {
            let raw = d.split(": ").nth(1).unwrap_or_default();
            let bare = raw
                .trim_start_matches('&')
                .trim_start_matches("Option<")
                .trim_start_matches("Vec<")
                .trim_end_matches('>')
                .trim_start_matches('&')
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .to_string();
            if !bare.is_empty() && !object_done.contains(&bare) {
                object_todo.push_back(bare);
            }
        }
        object_todo.extend(reached.into_iter().filter(|x| !object_done.contains(x)));
    }
    // Phase 2: build the code for every candidate, but keep it aside: only the
    // ones an exposed method can actually hand out are worth emitting.
    // Phase 2, with the registry complete: strict mapping becomes an object,
    // partial mapping becomes a read-only class. Deciding this in phase 1 got it
    // wrong, because a nested type may not have been discovered yet.
    let mut object_parts: BTreeMap<String, TokenStream> = BTreeMap::new();
    for t in vocab.objects.clone() {
        let source = if struct_fields_opt(&usvg_files, &t).is_some() {
            &usvg_files
        } else {
            &fontdb_files
        };
        let root = object_root[&t].clone();
        let (code, dropped, _) = object_struct(source, &t, &vocab, &modules, &root);
        if code.is_empty() {
            vocab.objects.remove(&t);
            let (vcode, _, _) = value_class(source, &t, &vocab, &modules, &root);
            if vcode.is_empty() {
                continue;
            }
            vocab.values.insert(t.clone());
            object_parts.insert(t.clone(), vcode);
            report!(
                "{t}: partial mapping, emitted as read-only class {}",
                value_class_ident(&t)
            );
            for d in dropped {
                report!("{t} member not exposed: {d}");
            }
        } else {
            object_parts.insert(t.clone(), code);
        }
    }

    let (fields, skipped_fields) = map_struct(&usvg_files, "Options", &vocab, precision_max);
    let (write_fields, skipped_write) =
        map_struct(&usvg_files, "WriteOptions", &vocab, precision_max);

    // Fixpoint: start from the handle types the wrapped impls return, generate a
    // class for each, then follow the handles *those* classes return. The type
    // graph is finite; the cap is only a runaway guard.
    const RESERVED: [&str; 8] = [
        "Resvg",
        "SvgNode",
        "BBox",
        "Matrix",
        "Dimensions",
        "RawImage",
        "RenderOptions",
        "RenderParams",
    ];
    let handles_of = |set: &BTreeSet<String>| -> BTreeSet<String> {
        set.iter()
            .filter_map(|t| match vocab.classify(t) {
                Some(Js::Handle(x)) | Some(Js::HandleList(x)) => Some(x),
                _ => None,
            })
            .collect()
    };
    let mut todo: Vec<String> = handles_of(&referenced).into_iter().collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut wrapper_code = TokenStream::new();
    while let Some(t) = todo.pop() {
        if !done.insert(t.clone()) || done.len() > 24 {
            continue;
        }
        let name = wrapper_ident(&t).to_string();
        if RESERVED.contains(&name.as_str()) {
            report!("handle {t} skipped: name {name} is taken");
            continue;
        }
        // `fontdb::Database` maps onto the hand-written FontDatabase class.
        if t == "fontdb::Database" {
            continue;
        }
        let (code, skipped, reached) = wrapper_class(&usvg_files, &t, &vocab, &modules);
        if code.is_empty() {
            report!("handle {t} skipped: nothing mappable on it");
        } else {
            wrapper_code.extend(code);
        }
        for s in skipped {
            // `root()` is served by the generated `children()` view above.
            let verb = if s.starts_with("root ") {
                "covered by the content view"
            } else {
                "not exposed"
            };
            report!("usvg::{t} method {verb}: {s}");
        }
        todo.extend(reached.into_iter().filter(|x| !done.contains(x)));
    }
    report!(
        "wrapper classes generated: {}",
        done.iter().cloned().collect::<Vec<_>>().join(", ")
    );

    // One declarative table instead of four call sites: adding a wrapped
    // `impl` block is a single row.
    let node_prologue = quote!(let __n = self.node()?;);
    struct Pass<'a> {
        label: &'a str,
        target: &'a str,
        files: &'a [syn::File],
        ty: &'a str,
        receiver: TokenStream,
        skip: &'a [&'a str],
        prologue: Option<&'a TokenStream>,
    }
    let passes = [
        Pass {
            label: "fontdb::Database",
            target: "FontDatabase",
            files: &fontdb_files,
            ty: "Database",
            receiver: quote!(self.inner),
            skip: &[],
            prologue: None,
        },
        Pass {
            label: "usvg::Tree",
            target: "Resvg",
            files: &usvg_files,
            ty: "Tree",
            receiver: quote!(self.tree),
            skip: &[],
            prologue: None,
        },
        Pass {
            label: "usvg::Group",
            target: "Resvg",
            files: &usvg_files,
            ty: "Group",
            receiver: quote!(self.tree.root()),
            skip: &["isolate", "should_isolate", "id"],
            prologue: None,
        },
        Pass {
            label: "usvg::Node",
            target: "SvgNode",
            files: &usvg_files,
            ty: "Node",
            receiver: quote!(__n),
            skip: &["subroots"],
            prologue: Some(&node_prologue),
        },
    ];

    let mut generated: BTreeMap<&str, TokenStream> = BTreeMap::new();
    let mut taken: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    // Reported once the template exists: whether a skipped method is "covered"
    // is a question about the template, which is built further down.
    let mut skips: Vec<(&str, Vec<String>)> = Vec::new();
    for p in &passes {
        let names = taken.entry(p.target).or_default();
        let (code, skipped) = map_methods(
            &MethodPass {
                files: p.files,
                ty: p.ty,
                receiver: &p.receiver,
                skip: p.skip,
                prologue: p.prologue,
                readonly: false,
            },
            &vocab,
            names,
        );
        skips.push((p.label, skipped));
        generated.insert(p.ty, code);
    }
    // Prune: an object type nothing returns is dead TS surface. Start from the
    // generated method bodies, then follow objects nested in kept objects.
    let mentions = |code: &TokenStream, name: &str| -> bool {
        format!(" {code} ").contains(&format!(" {name} "))
    };
    let mut kept: BTreeSet<String> = BTreeSet::new();
    let all_data: BTreeSet<String> = vocab.objects.union(&vocab.values).cloned().collect();
    for name in all_data.clone() {
        // A value class is referenced under its JS name, not the Rust one.
        let js = value_class_ident(&name).to_string();
        let used = generated
            .values()
            .any(|c| mentions(c, &name) || mentions(c, &js))
            || mentions(&wrapper_code, &name)
            || mentions(&wrapper_code, &js);
        if used {
            kept.insert(name);
        }
    }
    loop {
        let mut added = false;
        for name in all_data.clone() {
            if kept.contains(&name) {
                continue;
            }
            if kept
                .iter()
                .any(|k| object_parts.get(k).is_some_and(|c| mentions(c, &name)))
            {
                kept.insert(name);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let dropped: Vec<String> = all_data.difference(&kept).cloned().collect();
    if !dropped.is_empty() {
        report!("object types pruned as unreachable: {}", dropped.join(", "));
    }
    let object_code: TokenStream = kept
        .iter()
        .filter_map(|k| object_parts.get(k).cloned())
        .collect();
    report!(
        "data types emitted: {}",
        kept.iter().cloned().collect::<Vec<_>>().join(", ")
    );

    let fontdb_methods = &generated["Database"];
    let tree_methods = &generated["Tree"];
    let group_methods = &generated["Group"];
    let node_methods = &generated["Node"];

    for s in skipped_fields {
        report!("usvg::Options field not exposed: {s}");
    }
    for s in skipped_write {
        report!("usvg::WriteOptions field not exposed: {s}");
    }

    let decls: Vec<_> = fields.iter().map(|f| &f.decl).collect();
    let assigns: Vec<_> = fields.iter().map(|f| &f.assign).collect();
    let write_decls: Vec<_> = write_fields.iter().map(|f| &f.decl).collect();
    let write_assigns: Vec<_> = write_fields.iter().map(|f| &f.assign).collect();

    // Everything the passes above produced, in one value: `template` is called
    // twice and only the method lists differ between the two.
    let fragments = Fragments {
        enums,
        object_code,
        wrapper_code,
        decls,
        assigns,
        write_decls,
        write_assigns,
    };
    // What the template covers is derived from the template: build it once with
    // no generated methods, and read its own method names off it.
    let empty = TokenStream::new();
    let hand = template_fns(&template(&fragments, &empty, &empty, &empty, &empty));
    let defined: BTreeSet<&String> = hand.values().flatten().collect();
    for (from, to) in RENAMED {
        assert!(
            defined.iter().any(|n| n.as_str() == *to),
            "RENAMED says `{from}` is covered by `{to}`, but the template defines no `{to}`"
        );
    }
    for (label, skipped) in &skips {
        for s in skipped {
            let name = s.split(' ').next().unwrap_or_default();
            let renamed = RENAMED.iter().find(|(from, _)| *from == name);
            let verb = match renamed {
                Some((_, to)) => format!("covered by the template as `{to}`"),
                None if defined.iter().any(|n| n.as_str() == name) => {
                    "covered by the template, not generated".to_string()
                }
                None => "not exposed".to_string(),
            };
            report!("{label} method {verb}: {s}");
        }
    }

    let mut code = template(
        &fragments,
        fontdb_methods,
        tree_methods,
        group_methods,
        node_methods,
    );

    // Async twins: the template marks a Send-safe core, the rule writes the
    // ceremony. The marker is stripped on the way out -- rustc has never heard
    // of it.
    {
        let mut file: syn::File =
            syn::parse2(code.clone()).expect("the emitter template is not valid Rust");
        let twins = async_twins(&mut file);
        for t in &twins {
            report!(
                "async twin generated: {}::{} -> {} (Promise<{}>)",
                ty_str(&t.target),
                t.core,
                t.public,
                t.js
            );
        }
        let ceremony = emit_twins(&twins);
        code = quote! { #file #ceremony };
    }

    // Nothing above prevents a future upstream method from colliding with a
    // name the template already uses: rustc would say so, from inside a
    // generated file. Say it here instead.
    for (target, names) in template_fns(&code) {
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        for n in &names {
            assert!(
                seen.insert(n),
                "{target}::{n} is emitted twice: the template defines it and a \
                 pass generated it too. Add it to RENAMED, or skip it in the pass."
            );
        }
    }

    const HEADER: &str = concat!(
        "// @generated by build.rs from the resvg/usvg/fontdb sources. DO NOT EDIT.\n",
        "// Regenerate with `touch build.rs && cargo build`.\n",
        "#![allow(clippy::all, dead_code)]\n",
    );
    let text = format!("{HEADER}{}", code);

    // Staging copy, handy for inspection: $OUT_DIR/bindings.rs
    let staged = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    fs::write(&staged, &text).unwrap();
    rustfmt(&staged);

    // Real target. It has to be the crate root itself: `napi-derive` keeps a
    // global struct registry and requires `#[napi] struct` to expand before
    // `#[napi] impl`, which `include!(...)` does not preserve.
    let dest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("src/lib.rs");
    let formatted = fs::read_to_string(&staged).unwrap_or(text);
    // Write only on change, otherwise the touched mtime re-triggers the build.
    if fs::read_to_string(&dest).ok().as_deref() != Some(formatted.as_str()) {
        fs::write(&dest, &formatted).unwrap();
    }
}

fn rustfmt(path: &Path) {
    // Best effort: readable output when rustfmt is around, ignored otherwise.
    let _ = std::process::Command::new("rustfmt")
        .arg("--edition=2024")
        .arg(path)
        .status();
}
