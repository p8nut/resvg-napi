//! What a Rust type means on the JavaScript side.
//!
//! Phase 2. Nothing here emits code -- it answers questions. What shape does
//! this type map to? Is it a newtype over `f32`? An alias? An `Arc`-held
//! definition with an `id()` to name it by? An enum whose variants carry
//! payloads?
//!
//! Recognition is by *shape* wherever it can be: a newtype is a type with a
//! `get(&self) -> f32` and no other argument, not a name on a list. The few
//! places that do name a type say why, and each is an API choice rather than a
//! description of upstream.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use syn::{Fields, ImplItem, Item, ReturnType, Type};

use crate::sources::is_pub;

/// Normalised textual form of a type, e.g. `Option<std::path::PathBuf>`.
pub fn ty_str(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

// ---------------------------------------------------------------------------
// 2 + 3. usvg::Options -> #[napi(object)] RenderOptions
// ---------------------------------------------------------------------------

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
pub enum Js {
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
    // `&[f32]` / `Vec<f32>`, e.g. a dash pattern.
    F32List,
    // `i32`, which napi maps to a JS number directly. A turbulence seed.
    I32,
    // `u16`, widened to u32 on the JS side: a font weight, an axis count.
    U16,
    // `[u8; 4]`: an OpenType axis tag, four ASCII bytes -- `b"wght"`. A string
    // on the JS side, which is how every font tool spells it.
    Tag4,
    // `tiny_skia_path::Path`: outside usvg, and a verb/point stream rather than
    // a struct, so the template flattens it into segments.
    PathData,
    // An upstream enum carrying payloads, emitted as a discriminated union.
    PayloadEnum(String),
    PayloadEnumList(String),
}

/// One upstream enum whose variants carry payloads. usvg has sixteen; a rule
/// cannot invent their JS shape, but it can apply one convention to all of
/// them, which is what this describes: a discriminated union, tagged `type`,
/// one struct per payload variant plus one shared struct for the unit ones.
///
/// Everything here is syntactic, so it is known before the object registry is:
/// the payload *types* are classified later, when the structs are emitted.
/// What one variant of a payload enum carries.
#[derive(Clone)]
pub enum Payload {
    /// `Identity` -- nothing but its discriminant.
    None,
    /// `Table(Vec<f32>)` -- one unnamed field, exposed as `value`.
    Value(String),
    /// `Linear { slope: f32, intercept: f32 }` -- named fields, exposed under
    /// their own names, which is a better shape than wrapping them in `value`.
    Fields(Vec<(String, String)>),
}

#[derive(Clone)]
pub struct PayloadEnum {
    /// Variant name and what it carries.
    pub variants: Vec<(String, Payload)>,
}

impl PayloadEnum {
    /// `Kind` -> `KindBlend`, the struct for one payload variant.
    pub fn variant_ident(&self, enum_name: &str, variant: &str) -> proc_macro2::Ident {
        // `SVG` becomes `Svg`, because napi normalises a type name to PascalCase
        // on the way out and the union type here has to match what it wrote.
        // It aliases the original spelling back for classes -- `export type
        // ImageKindPNG = ImageKindPng` -- but not for objects, so `ImageKindSVG`
        // named a type that did not exist.
        let v = if variant.len() > 1
            && variant
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            let mut c = variant.chars();
            c.next().into_iter().collect::<String>() + &c.as_str().to_lowercase()
        } else {
            variant.to_string()
        };
        format_ident!("{enum_name}{v}")
    }

    /// The struct the unit variants share, or None when there are none.
    pub fn unit_ident(&self, enum_name: &str) -> Option<proc_macro2::Ident> {
        self.variants
            .iter()
            .any(|(_, p)| matches!(p, Payload::None))
            .then(|| format_ident!("{enum_name}Plain"))
    }
}

impl PayloadEnum {
    /// The structs of the union, in declaration order, unit ones first.
    pub fn parts(&self, name: &str) -> Vec<proc_macro2::Ident> {
        let mut v: Vec<_> = self.unit_ident(name).into_iter().collect();
        v.extend(
            self.variants
                .iter()
                .filter(|(_, p)| !matches!(p, Payload::None))
                .map(|(n, _)| self.variant_ident(name, n)),
        );
        v
    }

    /// `Either<A, B>` / `Either17<..>`, or the bare struct when there is one.
    pub fn either_ty(&self, name: &str) -> TokenStream {
        let parts = self.parts(name);
        match parts.len() {
            1 => quote!(#(#parts)*),
            2 => quote!(Either<#(#parts),*>),
            n => {
                let e = format_ident!("Either{n}");
                quote!(#e<#(#parts),*>)
            }
        }
    }

    /// `Either::A` / `Either17::C`, for the nth arm.
    pub fn either_arm(&self, name: &str, i: usize) -> TokenStream {
        let letter = format_ident!("{}", ((b'A' + i as u8) as char).to_string());
        let n = self.parts(name).len();
        let e = if n == 2 {
            format_ident!("Either")
        } else {
            format_ident!("Either{n}")
        };
        if n == 1 {
            quote!()
        } else {
            quote!(#e::#letter)
        }
    }

    /// Whether every payload maps to something an object field can hold. The
    /// generator's match is the authority on *how*; this answers *whether*, so
    /// a union that cannot be built whole is never classified in the first
    /// place and its member is reported unexposed instead.
    pub fn payload_maps(&self, vocab: &Vocab) -> bool {
        self.payload_blocker(vocab).is_none()
    }

    /// The first payload that stops the union being built, and why. Reported
    /// rather than silently dropped: a usvg upgrade adding a mappable payload
    /// should show up as a union appearing, not as a mystery.
    pub fn payload_blocker(&self, vocab: &Vocab) -> Option<String> {
        /// Whether one carried type can be a field of an `#[napi(object)]`.
        /// `sole` says the variant carries this and nothing else, which is the
        /// only shape `payload_enum_code` turns into a read-only class. A named
        /// field beside others has to be an object field, and bytes cannot be
        /// one.
        fn field_of(vocab: &Vocab, p: &str, sole: bool) -> Result<(), String> {
            if let Some(Js::Handle(t)) = vocab.classify(p) {
                return if vocab.with_id.contains(&t) {
                    Ok(())
                } else {
                    Err(format!("{t} has no id() to name it by"))
                };
            }
            // A payload that is itself a payload enum would recurse through
            // `classify`; not a shape usvg uses.
            if vocab.payload.contains_key(&vocab.resolve(p)) {
                return Err("it is itself a payload enum".into());
            }
            // A payload naming a public struct the registry dropped carries nothing
            // mappable: `filter::Image` has one method, `root() -> &Group`, and a
            // Group is a handle question. The variant keeps its discriminant and
            // loses its value, because there is no value to give it.
            if vocab.classify(p).is_none() && vocab.structs.contains(bare(p)) {
                return Ok(());
            }
            match vocab.classify(p) {
                Some(
                    Js::Str
                    | Js::F32
                    | Js::F64
                    | Js::U32
                    | Js::U8
                    | Js::U16
                    | Js::I32
                    | Js::F32List
                    | Js::Scalar
                    | Js::Bool
                    | Js::Enum
                    | Js::Object(_),
                ) => Ok(()),
                // A read-only class is not a field: napi needs Clone and
                // FromNapiValue of one, and a class has neither.
                Some(Js::Value(t)) => Err(format!("{t} maps only partially")),
                // A union variant is an `#[napi(object)]`, and napi wants Clone
                // of every field. `Buffer` is a handle into the JS heap and has
                // none, so image bytes cannot be a field of one -- they would
                // need the variant to be a read-only class with a getter, which
                // the union machinery does not build.
                // Bytes cannot be an object *field* -- neither `Buffer` nor
                // `Uint8Array` is `Clone`, and napi requires that of every
                // field. They can be a getter, so the variant carrying them is
                // emitted as a read-only class instead. See
                // `payload_enum_code`.
                // Bytes alone become a read-only class with a getter. Bytes
                // beside other named fields have nowhere to go, and saying
                // otherwise here left `classify` committed to a union that
                // `payload_enum_code` then refused to emit: a dangling
                // `*_to_js` call and a crate that does not compile, with the
                // report line explaining why never printed because `napi build`
                // failed first.
                Some(Js::Bytes) if sole => Ok(()),
                Some(Js::Bytes) => Err(
                    "being bytes, it can only be carried alone: a variant with named fields \
                     is an object, and napi requires Clone of every field"
                        .into(),
                ),
                other => Err(format!("it maps to {other:?}")),
            }
        }

        self.variants.iter().find_map(|(n, p)| {
            let carried: Vec<(Option<&str>, &String)> = match p {
                Payload::None => Vec::new(),
                Payload::Value(t) => vec![(None, t)],
                Payload::Fields(f) => f.iter().map(|(k, t)| (Some(k.as_str()), t)).collect(),
            };
            let sole = matches!(p, Payload::Value(_));
            carried.into_iter().find_map(|(field, t)| {
                field_of(vocab, t, sole).err().map(|why| match field {
                    Some(f) => format!("{n}.{f} carries {t}, and {why}"),
                    None => format!("{n} carries {t}, and {why}"),
                })
            })
        })
    }

    pub fn conv_ident(&self, name: &str) -> proc_macro2::Ident {
        let mut snake = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                snake.push('_');
            }
            snake.extend(c.to_lowercase());
        }
        format_ident!("{snake}_to_js")
    }
}

/// Types with an `id(&self) -> &str`, directly or through a Deref the sources
/// declare. An `Arc<T>` payload cannot be an object field, but if T has an id
/// it can be named by one -- the way the document itself refers to a paint
/// server with `url(#id)`.
pub fn types_with_id(files: &[syn::File]) -> BTreeSet<String> {
    let mut direct = BTreeSet::new();
    let mut derefs: BTreeMap<String, String> = BTreeMap::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        let target = ty_str(&imp.self_ty);
        if let Some(tr) = &imp.trait_ {
            if tr.0.segments.last().is_some_and(|seg| seg.ident == "Deref") {
                for it in &imp.items {
                    if let ImplItem::Type(t) = it {
                        derefs.insert(target.clone(), ty_str(&t.ty));
                    }
                }
            }
            continue;
        }
        if imp.items.iter().any(|it| {
            matches!(it, ImplItem::Fn(f)
                if is_pub(&f.vis) && f.sig.ident == "id" && f.sig.inputs.len() == 1)
        }) {
            direct.insert(target);
        }
    }
    // one hop is enough: LinearGradient -> BaseGradient -> id()
    let mut out = direct.clone();
    for (from, to) in derefs {
        if direct.contains(&to) {
            out.insert(from);
        }
    }
    out
}

/// Payload enums, found by shape. Skips any with a variant carrying more than
/// one field or named fields: those have no single obvious `value`, and saying
/// so in the report beats guessing.
pub fn payload_enums(
    files: &[syn::File],
    // The module these files contribute to, and the names usvg defines twice.
    // `filter::Kind::Image(Image)` means `filter::Image`, and nothing in the
    // type string says so -- the variant is written unqualified because it is
    // in the same module.
    module: &str,
    dups: &BTreeSet<String>,
) -> BTreeMap<String, PayloadEnum> {
    let mut out = BTreeMap::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Enum(e) = item else { continue };
        if !is_pub(&e.vis) {
            continue;
        }
        let qualify = |t: String| {
            if !module.is_empty() && dups.contains(&t) {
                format!("{module}::{t}")
            } else {
                t
            }
        };
        let mut variants = Vec::new();
        let mut usable = false;
        for v in &e.variants {
            match &v.fields {
                Fields::Unit => variants.push((v.ident.to_string(), Payload::None)),
                Fields::Unnamed(f) if f.unnamed.len() == 1 => {
                    usable = true;
                    let ty = qualify(ty_str(&f.unnamed[0].ty));
                    variants.push((v.ident.to_string(), Payload::Value(ty)));
                }
                Fields::Named(f) => {
                    usable = true;
                    let fields = f
                        .named
                        .iter()
                        .filter(|x| is_pub(&x.vis) || true)
                        .filter_map(|x| {
                            x.ident
                                .as_ref()
                                .map(|i| (i.to_string(), qualify(ty_str(&x.ty))))
                        })
                        .collect();
                    variants.push((v.ident.to_string(), Payload::Fields(fields)));
                }
                // A tuple of more than one field has no name to give either
                // half, so there is nothing honest to call them.
                _ => {
                    usable = false;
                    break;
                }
            }
        }
        if usable {
            out.insert(e.ident.to_string(), PayloadEnum { variants });
        }
    }
    out
}

/// Everything the classifier needs to know about the upstream crates, all of it
/// derived: enums worth mirroring, `f32` newtypes, and `pub type` aliases.
#[derive(Default)]
pub struct Vocab {
    /// Every public struct name upstream, so a payload that names one the
    /// registry dropped can be told from a payload that is not a struct at all.
    pub structs: BTreeSet<String>,
    /// Types that can be named by an id, for an `Arc<T>` payload.
    pub with_id: BTreeSet<String>,
    /// Upstream enums carrying payloads, by name. Syntactic, so populated with
    /// the rest of the vocabulary rather than after the object registry.
    pub payload: BTreeMap<String, PayloadEnum>,
    pub enums: BTreeSet<String>,
    pub scalars: BTreeSet<String>,
    pub aliases: BTreeMap<String, String>,
    pub ints: BTreeSet<String>,
    pub objects: BTreeSet<String>,
    pub values: BTreeSet<String>,
}

impl Vocab {
    /// `Opacity` -> `NormalizedF32` -> ... until it stops being an alias.
    pub fn resolve(&self, ty: &str) -> String {
        // `crate::FontVariation` and `FontVariation` are the same type: usvg
        // spells some of its own members with the prefix and the rest without,
        // and nothing downstream cares which.
        // `crate::FontVariation` and `FontVariation` are the same type: usvg
        // spells some of its own members with the prefix and the rest without.
        //
        // Lifetimes are *not* stripped here on purpose: `ty_str` drops
        // whitespace, so `&'a str` arrives as `&'astr` and there is no way to
        // tell the lifetime from the type that follows it. That is why
        // `Family::Name` is reported unmapped rather than guessed at.
        let mut t = ty.replace("crate::", "");
        for _ in 0..8 {
            match self.aliases.get(&t) {
                Some(next) if *next != t => t = next.clone(),
                _ => break,
            }
        }
        t
    }

    pub fn classify(&self, ty: &str) -> Option<Js> {
        let t = self.resolve(ty);
        if let Some(js) = classify(&t, &self.enums, &self.scalars) {
            return Some(js);
        }
        if self.ints.contains(&t) {
            return Some(Js::IntNewtype(t));
        }
        // `&Paint` and `Paint` are the same enum; the registry is keyed bare.
        let bare = t.trim_start_matches('&').to_string();
        if let Some(p) = self.payload.get(&bare) {
            return p.payload_maps(self).then_some(Js::PayloadEnum(bare));
        }
        // `&[BaselineShift]`: a list of one, which reads the same way.
        if let Some(inner) = t
            .trim_start_matches('&')
            .strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .map(|r| self.resolve(r.trim_start_matches('&')))
        {
            if let Some(p) = self.payload.get(&inner) {
                return p.payload_maps(self).then_some(Js::PayloadEnumList(inner));
            }
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
pub fn type_aliases(files: &[syn::File]) -> BTreeMap<String, String> {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(|i| match i {
            Item::Type(t) if is_pub(&t.vis) => Some((t.ident.to_string(), ty_str(&t.ty))),
            _ => None,
        })
        .collect()
}

pub fn classify(
    ty: &str,
    known_enums: &BTreeSet<String>,
    scalars: &BTreeSet<String>,
) -> Option<Js> {
    Some(match ty {
        "Transform" => Js::Matrix,
        "f32" => Js::F32,
        "f64" => Js::F64,
        "u32" => Js::U32,
        "u8" => Js::U8,
        "u16" => Js::U16,
        "i32" => Js::I32,
        "bool" => Js::Bool,
        "usize" => Js::Count,
        "String" | "&str" => Js::Str,
        "Option<String>" => Js::OptStr,
        "Option<std::path::PathBuf>" | "Option<PathBuf>" => Js::OptPath,
        "Vec<String>" => Js::StrList,
        "Vec<f32>" | "&[f32]" | "[f32]" | "Vec<f64>" | "&[f64]" | "[f64]" => Js::F32List,
        // An Arc around bytes is shared *data*, not a shared entity: it has no
        // identity to name it by, and treating it as a handle is what made
        // `ImageKind`'s image payloads unmappable.
        "Vec<u8>" | "&[u8]" | "Arc<Vec<u8>>" | "&Arc<Vec<u8>>" | "std::sync::Arc<Vec<u8>>" => {
            Js::Bytes
        }
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
        "tiny_skia_path::Path" | "&tiny_skia_path::Path" => Js::PathData,
        "[u8;4]" | "&[u8;4]" | "[u8; 4]" => Js::Tag4,
        other if known_enums.contains(other) => Js::Enum,
        // Payload enums are classified by name here and given their union
        // further down; `map_enums` only mirrors the unit ones.
        other if scalars.contains(other) => Js::Scalar,
        _ => return None,
    })
}

/// Second classification pass, for types that need the object registry: it is
/// built after the first pass, so it cannot live in `classify`.
pub fn classify_object(ty: &str, objects: &BTreeSet<String>) -> Option<Js> {
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
pub fn int_newtypes(files: &[syn::File]) -> BTreeSet<String> {
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
pub fn f32_newtypes(files: &[syn::File]) -> BTreeSet<String> {
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
                    // The receiver and nothing else. `ConvolveMatrixData` has
                    // `get(&self, x: u32, y: u32) -> f32` -- a matrix accessor,
                    // not a newtype unwrapping itself -- and counting it as a
                    // scalar made the whole type invisible to discovery.
                    && f.sig.inputs.len() == 1
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
/// Every type named by a field of a public struct.
///
/// `returned_types` finds what methods hand back, which is how usvg's enums get
/// discovered -- they are all behind accessors. fontdb's are not:
/// `FaceInfo.style` is a plain public field, so `Style` was never in the wanted
/// set and the enum pass skipped it, leaving object discovery to report it as
/// "not a public struct" and drop the field.
pub fn field_types(files: &[syn::File]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Struct(st) = item else { continue };
        if !is_pub(&st.vis) {
            continue;
        }
        for f in &st.fields {
            let t = ty_str(&f.ty);
            out.insert(
                t.trim_start_matches('&')
                    .trim_start_matches("Option<")
                    .trim_end_matches('>')
                    .to_string(),
            );
        }
    }
    out
}

pub fn returned_types(files: &[syn::File], types: &[&str]) -> BTreeSet<String> {
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

pub fn static_array_len(files: &[syn::File], name: &str) -> usize {
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

pub fn struct_fields_opt<'a>(files: &'a [syn::File], ty: &str) -> Option<&'a Fields> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(&s.fields),
        _ => None,
    })
}

pub fn struct_fields<'a>(files: &'a [syn::File], ty: &str) -> &'a syn::FieldsNamed {
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
pub fn struct_field_types(files: &[syn::File], ty: &str) -> BTreeSet<String> {
    struct_fields(files, ty)
        .named
        .iter()
        .filter(|f| is_pub(&f.vis))
        .map(|f| ty_str(&f.ty))
        .collect()
}

pub fn docs(attrs: &[syn::Attribute]) -> Vec<TokenStream> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .map(|a| quote!(#a))
        .collect()
}

/// Upstream path of each type name, derived from the file it lives in.
pub fn upstream_modules(
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
            // A name declared both at the crate root and inside a module
            // resolves to the root one: `usvg::Image` is what `Image` means,
            // and `filter::Image` says its module. First-wins made that depend
            // on which file the walk reached first, which put `Image` in
            // `filter` and left the tree's own image type unnameable.
            match out.entry(name) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(prefix.clone());
                }
                std::collections::btree_map::Entry::Occupied(mut e) if prefix.is_empty() => {
                    e.insert(String::new());
                }
                _ => {}
            }
        }
    }
    out
}

/// `usvg::filter::EdgeMode` or `usvg::BlendMode`, as appropriate.
pub fn upstream_path(
    name: &str,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> TokenStream {
    // A qualified key says its own module, and says it correctly: the map below
    // is keyed by bare name and cannot hold two `Image`s.
    if let Some((m, n)) = name.rsplit_once("::") {
        let (m, n) = (format_ident!("{}", m), format_ident!("{}", n));
        return quote!(#root::#m::#n);
    }
    let ident = format_ident!("{}", name);
    match modules.get(name) {
        Some(m) if !m.is_empty() => {
            let m = format_ident!("{}", m);
            quote!(#root::#m::#ident)
        }
        _ => quote!(#root::#ident),
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Arg {
    Str,
    PathStr,
    Bytes,
    // fontdb's `ID` is an opaque slotmap key with no JS form. A `FontFace`
    // carries the `FaceInfo` it came from, so it can stand in for the key.
    Face,
}

pub enum Ret {
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
pub fn deref_target(files: &[syn::File], ty: &str) -> Option<String> {
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

/// Naming decision: `FaceInfo` reads as `FontFace` on the JS side.
/// The tree itself. Group, Node and Tree are what a handle exists for, and
/// following one into the data registry drags every definition type behind it --
/// `Text::flattened` and `filter::Image::root` both hand back a Group, and
/// `ImageKind` carries a Tree. Filtered both as a member and as a candidate
/// seed, because either route reaches the same place.
pub const NODE_TYPES: [&str; 3] = ["Group", "Node", "Tree"];

/// The upstream name inside a registry key: `filter::Image` -> `Image`.
///
/// A key carries the module so that two types of the same name stay distinct;
/// every lookup into the sources wants the name alone.
pub fn bare(key: &str) -> &str {
    key.rsplit_once("::").map(|(_, n)| n).unwrap_or(key)
}

/// The JS name of a data type, whatever its registry key.
///
/// One function owns this because the naming has three rules and they were
/// spread over eighteen call sites, only some of which knew about the rename:
/// `Js::Object` built the ident with a bare `format_ident!` while `Js::Value`
/// went through here, so an object called `FaceInfo` would have been emitted
/// under that name. It only worked because FaceInfo maps partially.
///
/// A key can be module-qualified. usvg defines `Image` twice -- `tree/filter.rs`
/// and `tree/mod.rs` -- so the two cannot share a bare key without the path
/// resolving to one and the members to the other.
pub fn data_ident(ty: &str) -> proc_macro2::Ident {
    if let Some((module, name)) = ty.rsplit_once("::") {
        let head = module.rsplit("::").next().unwrap_or(module);
        let mut camel = String::new();
        let mut up = true;
        for c in head.chars() {
            if c == '_' {
                up = true;
            } else if up {
                camel.extend(c.to_uppercase());
                up = false;
            } else {
                camel.push(c);
            }
        }
        return format_ident!("{camel}{name}");
    }
    format_ident!("{}", if ty == "FaceInfo" { "FontFace" } else { ty })
}

/// JS name for a mapped enum.
///
/// `Style` alone says nothing next to usvg's own `FontStyle`, and the two would
/// sit side by side in the exports with identical variants: one is what a text
/// run asked for, the other what a loaded face is. Renamed for the same reason
/// `FaceInfo` is emitted as `FontFace`.
pub fn enum_ident(name: &str) -> proc_macro2::Ident {
    match name {
        "Style" => format_ident!("FontFaceStyle"),
        _ => format_ident!("{}", name),
    }
}

/// JS class name for an Arc-held usvg definition: `filter::Filter` -> `Filter`.
pub fn wrapper_ident(ty: &str) -> proc_macro2::Ident {
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
pub fn wrapper_path(ty: &str, modules: &BTreeMap<String, String>) -> TokenStream {
    if ty.contains("::") {
        let segs: Vec<_> = ty.split("::").map(|s| format_ident!("{}", s)).collect();
        quote!(usvg::#(#segs)::*)
    } else {
        upstream_path(ty, modules, &quote!(usvg))
    }
}

/// `Result<T>` -> `T`.
pub fn result_inner(ty: &Type) -> Option<Type> {
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

/// `render_png` -> `renderPng`, the name napi gives it in JS.
pub fn lower_camel(s: &str) -> String {
    // An all-caps name is an acronym, not camel case: `PNG` is `png`, and
    // lowering only the first letter would give `pNG`. usvg spells
    // `ImageKind::PNG`, `JPEG`, `GIF` and `WEBP` that way.
    if s.len() > 1
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return s.to_lowercase();
    }
    let camel = heck_camel(s);
    let mut c = camel.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `resvg_png_bytes` -> `ResvgPngBytes`.
pub fn heck_camel(s: &str) -> String {
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
