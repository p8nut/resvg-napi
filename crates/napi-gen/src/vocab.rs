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

use convert_case::{Case, Casing};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use syn::{Fields, ImplItem, Item, ReturnType, Type};

use crate::config;
use crate::sources::{is_pub, self_ident};

/// Normalised textual form of a type, e.g. `Option<std::path::PathBuf>`.
pub fn ty_str(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

/// What a Rust type becomes on the JS side.
///
/// Recognition of Rust types lives here, once. The field mapper and the method
/// mapper then each decide how to *emit* a given `Js` -- an assignment in one
/// case, a wrapper body in the other.
#[derive(Clone, PartialEq, Debug)]
pub enum Js {
    F32,
    U32,
    U8,
    Bool,
    Count,
    Str,
    OptStr,
    OptPath,
    // `PathBuf` / `&Path`: a string on the JS side, lossy where the OS
    // path is not UTF-8.
    Path,
    StrList,
    Bytes,
    TryUnit,
    Enum,
    // A listed class held by value (`Copy`), by its upstream name: what the
    // class's `__from` wraps. A plain-data one is its `<Name>Object` where a
    // class cannot go (an object field, a union's payload).
    Class(String),
    OptClass(String),
    // A listed class not held by value, whose upstream type is `Clone`: a
    // copy of the borrowed value, given to the class's `__from`; or, for a
    // projected class reached from a projection (the `Tree` an image embeds,
    // the `Group` an `feImage` draws), the value borrowed in place
    // (`Vocab::steps_into`). Never an object field: a class is not `Clone`.
    Owned(String),
    // `Vec<(A, B)>` or `&[(A, B)]`: an `Array<[A, B]>`, each element mapped
    // as a union's payload is.
    TupleList(Vec<(String, Js)>),
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
    // `Config::iterated`: the list a method iterates, each item mapped as
    // its type is. (The item's type, the method.)
    Iterated(Box<Js>, String),
    // An upstream enum carrying payloads, emitted as a discriminated union.
    PayloadEnum(String),
    PayloadEnumList(String),
}

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
    /// `Line(Point, Point)` -- unnamed fields, exposed as `value`, an array
    /// of them: a tuple, as `Vec<(A, B)>` is one.
    Tuple(Vec<String>),
}

impl Payload {
    /// Whether it carries bytes: the variant is then a read-only class, a
    /// getter being the one place a `Buffer` can go (`payload_enum_code`).
    pub fn carries_bytes(&self, vocab: &Vocab) -> bool {
        self.carried()
            .iter()
            .any(|t| matches!(vocab.classify(t), Some(Js::Bytes)))
    }

    /// The types it carries.
    pub fn carried(&self) -> Vec<&String> {
        match self {
            Payload::None => Vec::new(),
            Payload::Value(t) => vec![t],
            Payload::Fields(f) => f.iter().map(|(_, t)| t).collect(),
            Payload::Tuple(ts) => ts.iter().collect(),
        }
    }
}

/// One upstream enum whose variants carry payloads. A crate may have dozens; a rule
/// cannot invent their JS shape, but it can apply one convention to all of
/// them, which is what this describes: a discriminated union, tagged `type`,
/// one struct per payload variant plus one shared struct for the unit ones.
///
/// Everything here is syntactic, so it is known before the object registry is:
/// the payload *types* are classified later, when the structs are emitted.
#[derive(Clone)]
pub struct PayloadEnum {
    /// Variant name and what it carries.
    pub variants: Vec<(String, Payload)>,
    /// The Rust path its crate is reached by, `toy` or a re-export such as
    /// `a::b`. Carried on the type because the emitter used to assume the
    /// first crate, which stops being true at the next one.
    pub root: String,
    /// The `#[cfg]`s of the variants that carry any: a variant upstream builds
    /// only under one of its features has its arm built under the same cfg
    /// (`cfg::of`), or the match names a variant that is not there.
    pub cfgs: BTreeMap<String, Vec<syn::Attribute>>,
}

impl PayloadEnum {
    /// Whether a variant carries a value its union's object can borrow from
    /// the enum (`Vocab::steps_into`): the union is then made from a
    /// projection of the enum, not a reference to it (`payload_enum_code`).
    pub fn projects(&self, vocab: &Vocab) -> bool {
        self.variants.iter().any(|(_, p)| match p {
            Payload::Value(ty) => vocab.classify(ty).is_some_and(|js| vocab.steps_into(&js)),
            _ => false,
        })
    }

    /// Whether a variant is emitted as a class (bytes, see `payload_enum_code`).
    /// A class is not `Clone`, so no `#[napi(object)]` field can hold the
    /// union: its holder has to stay a read-only class.
    pub fn holds_class(&self, vocab: &Vocab) -> bool {
        self.variants.iter().any(|(_, p)| match p {
            Payload::Value(ty) => matches!(
                vocab.classify(ty),
                Some(Js::Bytes | Js::Owned(_) | Js::Value(_))
            ),
            Payload::Tuple(_) => p.carries_bytes(vocab),
            _ => false,
        })
    }

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
            variant.to_case(Case::Pascal)
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
        let letter = format_ident!(
            "{}",
            char::from(b'A' + u8::try_from(i).expect("fewer than 26 letters"))
        );
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

    /// The first payload that stops the union being built, and why. Reported
    /// rather than silently dropped: an upstream upgrade adding a mappable payload
    /// should show up as a union appearing, not as a mystery.
    ///
    /// The generator's match is the authority on *how*; this answers
    /// *whether*, so a union that cannot be built whole is never classified in
    /// the first place and its member is reported unexposed instead.
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
            // `classify`; not a shape any wrapped crate uses yet.
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
            if let Some(Js::Class(t)) = vocab.classify(p) {
                return if vocab.plain.contains(&t) {
                    Ok(())
                } else {
                    Err(format!("{t} is a class that is not plain data"))
                };
            }
            match vocab.classify(p) {
                Some(
                    Js::Str
                    | Js::F32
                    | Js::U32
                    | Js::U8
                    | Js::U16
                    | Js::I32
                    | Js::F32List
                    | Js::Scalar
                    | Js::Bool
                    | Js::Enum
                    | Js::Path
                    | Js::Object(_),
                ) => Ok(()),
                // A class carried alone: the variant is an output-only
                // object holding it (`payload_enum_code`). Beside other
                // fields it would be an object field, which napi requires to
                // be Clone and a class is not.
                Some(Js::Value(_) | Js::Owned(_)) if sole => Ok(()),
                Some(Js::Value(t) | Js::Owned(t)) => {
                    Err(format!("{t} is a class, which can only be carried alone"))
                }
                // Bytes cannot be an object *field* -- neither `Buffer` nor
                // `Uint8Array` is `Clone`, and napi requires that of every
                // field. They can be a getter, so the variant carrying them is
                // emitted as a read-only class instead. See
                // `payload_enum_code`.
                //
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
                Payload::Fields(f) => f.iter().map(|(k, t)| (Some(k.as_str()), t)).collect(),
                p => p.carried().into_iter().map(|t| (None, t)).collect(),
            };
            // A tuple holding bytes is one getter too (`value`), so a class
            // may sit beside them. One without bytes is an object, whose
            // fields napi requires to be Clone.
            let sole = match p {
                Payload::Value(_) => true,
                Payload::Tuple(_) => p.carries_bytes(vocab),
                _ => false,
            };
            carried.into_iter().find_map(|(field, t)| {
                field_of(vocab, t, sole).err().map(|why| match field {
                    Some(f) => format!("{n}.{f} carries {t}, and {why}"),
                    None => format!("{n} carries {t}, and {why}"),
                })
            })
        })
    }

    /// `#[cfg(..)]` for the arm of variant `n`, translated (`cfg::of`), or
    /// nothing when it is built wherever the enum is.
    pub fn gate(&self, n: &str) -> TokenStream {
        self.cfgs
            .get(n)
            .map(|a| crate::cfg::attr(a))
            .unwrap_or_default()
    }

    pub fn conv_ident(&self, name: &str) -> proc_macro2::Ident {
        format_ident!("{}_to_js", name.to_case(Case::Snake))
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

/// An enum's variants as payloads, or why they cannot be read as one. Shared
/// by the payload registry and by `map_enums`, so an enum neither can use is
/// refused for one stated reason rather than dropped by both in silence.
pub fn payload_shape(
    e: &syn::ItemEnum,
    qualify: &dyn Fn(String) -> String,
) -> Result<Vec<(String, Payload)>, String> {
    let mut out = Vec::new();
    for v in &e.variants {
        let name = v.ident.to_string();
        let payload = match &v.fields {
            Fields::Unit => Payload::None,
            Fields::Unnamed(f) if f.unnamed.len() == 1 => {
                Payload::Value(qualify(ty_str(&f.unnamed[0].ty)))
            }
            Fields::Unnamed(f) if f.unnamed.is_empty() => {
                return Err(format!(
                    "{name}() has empty parens, which no JS shape mirrors"
                ));
            }
            // Several unnamed fields: a tuple, `value: [a, b]`.
            Fields::Unnamed(f) => {
                Payload::Tuple(f.unnamed.iter().map(|x| qualify(ty_str(&x.ty))).collect())
            }
            Fields::Named(f) => Payload::Fields(
                f.named
                    .iter()
                    .filter_map(|x| {
                        x.ident
                            .as_ref()
                            .map(|i| (i.to_string(), qualify(ty_str(&x.ty))))
                    })
                    .collect(),
            ),
        };
        out.push((name, payload));
    }
    Ok(out)
}

/// Payload enums, found by shape: an enum with a variant that carries
/// something.
pub fn payload_enums(
    files: &[syn::File],
    // The module these files contribute to, and the names upstream defines twice.
    // `filter::Kind::Image(Image)` means `filter::Image`, and nothing in the
    // type string says so -- the variant is written unqualified because it is
    // in the same module.
    module: &str,
    dups: &BTreeSet<String>,
    // The crate these files belong to, recorded on every enum found.
    root: &str,
) -> BTreeMap<String, PayloadEnum> {
    let mut out = BTreeMap::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Enum(e) = item else { continue };
        // A converter is that type's JS form (`Config::converters`): no
        // union of its own beside it, so none to classify a member as.
        if !is_pub(&e.vis) || config().converters.iter().any(|c| e.ident == c.ty) {
            continue;
        }
        let qualify = |t: String| {
            if !module.is_empty() && dups.contains(&t) {
                format!("{module}::{t}")
            } else {
                t
            }
        };
        // Unit variants alone are a plain enum, not a payload one.
        let Ok(variants) = payload_shape(e, &qualify) else {
            continue;
        };
        let usable = variants.iter().any(|(_, p)| !matches!(p, Payload::None));
        if usable {
            let cfgs = e
                .variants
                .iter()
                .filter_map(|v| {
                    let c: Vec<syn::Attribute> = v
                        .attrs
                        .iter()
                        .filter(|a| a.path().is_ident("cfg"))
                        .cloned()
                        .collect();
                    (!c.is_empty()).then(|| (v.ident.to_string(), c))
                })
                .collect();
            out.insert(
                e.ident.to_string(),
                PayloadEnum {
                    variants,
                    root: root.to_string(),
                    cfgs,
                },
            );
        }
    }
    out
}

/// The type a member is *about*, its wrappers peeled: `&`, `Result<..>`,
/// `Option<..>`, `Vec<..>`, `Box<..>`, a slice `[..]` and
/// `implIterator<Item=..>`, as often as they nest. Stops at `Arc<..>`: that marks a shared handle, and peeling it
/// would seed the handle's type as data. The path prefix is left on; callers
/// that want the bare name take it off. `ty_str` has removed the whitespace.
pub fn peel_type(t: &str) -> &str {
    let mut t = t;
    loop {
        let next = t
            .strip_prefix('&')
            .or_else(|| {
                ["Result<", "Option<", "Vec<", "Box<"]
                    .iter()
                    .find_map(|w| t.strip_prefix(w).and_then(|r| r.strip_suffix('>')))
            })
            .or_else(|| t.strip_prefix('[').and_then(|r| r.strip_suffix(']')))
            .or_else(|| {
                let r = t.strip_prefix("implIterator<Item=")?;
                let r = r.strip_suffix("+'_").unwrap_or(r);
                r.strip_suffix('>')
            });
        match next {
            Some(n) => t = n,
            None => return t,
        }
    }
}

/// Why a member or method was left out, as a record rather than a sentence:
/// the discovery fixpoint reads the type off it, the report reads the name,
/// and neither has to split a formatted string to get them back.
pub struct Skip {
    pub name: String,
    /// The upstream type the decision was about, when there is one.
    pub ty: Option<String>,
    pub reason: Reason,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A data member whose type has no JS form.
    Member,
    /// A union member some of whose variants are classes: no plain value.
    UnionWithClasses,
    /// An options field the template wires itself.
    HandWired,
    /// An options field with no JS form.
    Dropped,
    /// An option object's field with no JS form and no `Config::providers`
    /// entry: left out, so it keeps the `Default` value.
    Unprovided,
    NoReceiver,
    ConsumesSelf,
    Arg,
    Returns,
    NeedsMut,
    /// An `unsafe fn`: JS cannot uphold its contract.
    Unsafe,
    /// Returns a class whose JS name another namespace has, outside that
    /// class's own constructor and factories (`class::Class::shared_name`).
    SharedName,
}

impl Skip {
    pub fn new(name: &str, ty: Option<&str>, reason: Reason) -> Skip {
        Skip {
            name: name.to_string(),
            ty: ty.map(str::to_string),
            reason,
        }
    }
}

/// The report's wording, unchanged from when these were built as strings.
impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let (n, t) = (&self.name, self.ty.as_deref().unwrap_or_default());
        match self.reason {
            Reason::Member => write!(f, "{n}: {t}"),
            Reason::UnionWithClasses => write!(f, "{n}: {t} (a union with class variants)"),
            Reason::HandWired => write!(f, "{n}: {t} (handled manually by the template)"),
            Reason::Dropped => write!(f, "{n}: {t} (dropped)"),
            Reason::Unprovided => write!(f, "{n}: {t} (no provider: keeps the default)"),
            Reason::NoReceiver => write!(f, "{n} (no receiver)"),
            Reason::ConsumesSelf => write!(f, "{n} (consumes self)"),
            Reason::Arg => write!(f, "{n} (arg: {t})"),
            Reason::Returns => write!(f, "{n} (returns {t})"),
            Reason::NeedsMut => write!(f, "{n} (needs &mut self)"),
            Reason::Unsafe => write!(f, "{n} (unsafe)"),
            Reason::SharedName => write!(
                f,
                "{n} (returns {t}, a class whose JS name another namespace has: \
                 only its constructor and factories may)"
            ),
        }
    }
}

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
    /// The listed classes held by value (`Copy`) that a member may name by
    /// their upstream name: no discovery source defines that name itself
    /// (`Js::Class`).
    pub classes: BTreeSet<String>,
    /// Those of them that are plain data, with a `<Name>Object`.
    pub plain: BTreeSet<String>,
    /// The listed classes not held by value whose upstream type is `Clone`
    /// (`Js::Owned`): a member borrowing one hands out a copy.
    pub owned: BTreeSet<String>,
    /// Those of them that hold a projection (`Holding::Projected`): a member
    /// reached from a projection borrows one rather than copying it.
    pub projected: BTreeSet<String>,
}

impl Vocab {
    /// Whether a value of this kind, reached from a projection, is handed out
    /// as a projection one step further instead of a copy: a projected class,
    /// a read-only class (which holds a projection), and a union carrying
    /// either. Not a class JS can write (`Canvas`): a write would reach the
    /// owner, which a projection never writes.
    pub fn steps_into(&self, js: &Js) -> bool {
        match js {
            Js::Owned(t) => self.projected.contains(t),
            Js::Value(_) => true,
            Js::PayloadEnum(t) => self.payload[t].projects(self),
            _ => false,
        }
    }

    /// `Opacity` -> `NormalizedF32` -> ... until it stops being an alias.
    pub fn resolve(&self, ty: &str) -> String {
        // `crate::FontVariation` and `FontVariation` are the same type: upstream
        // spells some of its own members with the prefix and the rest without,
        // and nothing downstream cares which.
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
        let unref = t.trim_start_matches('&');
        if self.classes.contains(unref) {
            return Some(Js::Class(unref.to_string()));
        }
        if let Some(inner) = unref
            .strip_prefix("Option<")
            .and_then(|r| r.strip_suffix('>'))
            .map(|r| r.trim_start_matches('&'))
            && self.classes.contains(inner)
        {
            return Some(Js::OptClass(inner.to_string()));
        }
        if self.owned.contains(unref) {
            return Some(Js::Owned(unref.to_string()));
        }
        if let Some(it) = config().iterated.iter().find(|i| i.ty == unref) {
            return self
                .classify(it.item)
                .map(|item| Js::Iterated(Box::new(item), it.via.to_string()));
        }
        // `Vec<(A, B)>`, `&[(A, B)]`: every element must map.
        if let Some(inner) = unref
            .strip_prefix("Vec<")
            .and_then(|r| r.strip_suffix('>'))
            .or_else(|| unref.strip_prefix('[').and_then(|r| r.strip_suffix(']')))
            && let Some(elems) = tuple_elems(inner)
        {
            let mapped: Option<Vec<(String, Js)>> = elems
                .into_iter()
                .map(|e| self.classify(&e).map(|j| (e, j)))
                .collect();
            return mapped.map(Js::TupleList);
        }
        if let Some(js) = classify(&t, &self.enums, &self.scalars) {
            return Some(js);
        }
        if self.ints.contains(&t) {
            return Some(Js::IntNewtype(t));
        }
        // `&Paint` and `Paint` are the same enum; the registry is keyed bare.
        let bare = t.trim_start_matches('&').to_string();
        if let Some(p) = self.payload.get(&bare) {
            return p
                .payload_blocker(self)
                .is_none()
                .then_some(Js::PayloadEnum(bare));
        }
        // `&[BaselineShift]`: a list of one, which reads the same way.
        if let Some(inner) = t
            .trim_start_matches('&')
            .strip_prefix('[')
            .and_then(|r| r.strip_suffix(']'))
            .map(|r| self.resolve(r.trim_start_matches('&')))
            && let Some(p) = self.payload.get(&inner)
        {
            return p
                .payload_blocker(self)
                .is_none()
                .then_some(Js::PayloadEnumList(inner));
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
        // No `Transform` arm: `Vocab::matrices` answers that one by shape,
        // before this function is reached.
        "f32" => Js::F32,
        "u32" => Js::U32,
        "u8" => Js::U8,
        "u16" => Js::U16,
        "i32" => Js::I32,
        "bool" => Js::Bool,
        "usize" => Js::Count,
        "String" | "&str" => Js::Str,
        "Option<String>" => Js::OptStr,
        "Option<std::path::PathBuf>" | "Option<PathBuf>" => Js::OptPath,
        "std::path::PathBuf" | "PathBuf" | "&std::path::Path" | "&Path" => Js::Path,
        "Vec<String>" => Js::StrList,
        "Vec<f32>" | "&[f32]" | "[f32]" | "Vec<f64>" | "&[f64]" | "[f64]" => Js::F32List,
        // An Arc around bytes is shared *data*, not a shared entity: it has no
        // identity to name it by, and treating it as a handle is what made
        // `ImageKind`'s image payloads unmappable.
        "Vec<u8>" | "&[u8]" | "Arc<Vec<u8>>" | "&Arc<Vec<u8>>" | "std::sync::Arc<Vec<u8>>" => {
            Js::Bytes
        }
        s if shared_bytes(s) => Js::Bytes,
        s if s.starts_with("Result<(),") => Js::TryUnit,
        // Arc-held definitions can be owned by a wrapper class.
        s if s.starts_with("&[Arc<") && s.ends_with(">]") => {
            Js::HandleList(s[6..s.len() - 2].to_string())
        }
        s if s.starts_with("&Arc<") && s.ends_with('>') => {
            Js::Handle(s[5..s.len() - 1].to_string())
        }
        s if s.starts_with("Arc<") && s.ends_with('>') => Js::Handle(s[4..s.len() - 1].to_string()),
        s if config().tag4_types.contains(&s) => Js::Tag4,
        other if known_enums.contains(other) => Js::Enum,
        // Payload enums are classified by name here and given their union
        // further down; `map_enums` only mirrors the unit ones.
        other if scalars.contains(other) => Js::Scalar,
        _ => return None,
    })
}

/// `Arc<dyn AsRef<[u8]> + Send + Sync>`, by any path to `Arc` and in any
/// marker order: bytes a crate shares without saying what holds them (a
/// `Vec`, a file mapping). Read through `AsRef`, as any bytes are.
pub fn shared_bytes(ty: &str) -> bool {
    bare_arc(ty.trim_start_matches('&'))
        .strip_prefix("Arc<dynAsRef<[u8]>")
        .and_then(|r| r.strip_suffix('>'))
        .is_some_and(|markers| {
            markers
                .split('+')
                .all(|m| ["", "Send", "Sync", "'static"].contains(&m))
        })
}

/// `Arc` by its own name, whichever path spelled it: `alloc::sync::Arc<T>`
/// and `std::sync::Arc<T>` are `Arc<T>`.
pub fn bare_arc(ty: &str) -> String {
    ty.replace("alloc::sync::Arc<", "Arc<")
        .replace("std::sync::Arc<", "Arc<")
}

/// A type string as `ty_str` left it, made Rust again where dropping the
/// spaces fused two words (`dynAsRef`), with `Arc` spelled out in full: the
/// generated file imports neither `alloc` nor `std::sync`.
pub fn rust_src(ty: &str) -> String {
    bare_arc(ty)
        .replace("Arc<", "std::sync::Arc<")
        .replace("<dyn", "<dyn ")
        .replace("&dyn", "&dyn ")
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
        && objects.contains(inner)
    {
        return Some(Js::OptObject(inner.to_string()));
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
        if let Fields::Unnamed(u) = &s.fields
            && u.unnamed.len() == 1
        {
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

/// A tuple type's elements, `(A,B)` -> `[A, B]`: split at the top level, so
/// `(Vec<(A,B)>,C)` has two. None for anything but a tuple of two or more.
pub fn tuple_elems(t: &str) -> Option<Vec<String>> {
    let inner = t.strip_prefix('(')?.strip_suffix(')')?;
    let (mut out, mut depth, mut cur) = (Vec::new(), 0i32, String::new());
    for c in inner.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    (out.len() > 1).then_some(out)
}

/// What a member's type is about, [`peel_type`]'s answer, or each element's
/// when that is a tuple: `Vec<(String,Language)>` names both.
pub fn peeled(t: &str) -> Vec<String> {
    let p = peel_type(t);
    match tuple_elems(p) {
        Some(elems) => elems.iter().flat_map(|e| peeled(e)).collect(),
        None => vec![p.to_string()],
    }
}

/// Every type named by a field of a public struct.
///
/// `returned_types` finds what methods hand back, which is how enums behind
/// accessors get discovered. An enum held by a plain public field is not:
/// `FaceInfo.style` never put `Style` in the wanted set, so the enum pass
/// skipped it, leaving object discovery to report it as "not a public struct"
/// and drop the field.
pub fn field_types(files: &[syn::File]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Struct(st) = item else { continue };
        if !is_pub(&st.vis) {
            continue;
        }
        for f in &st.fields {
            out.extend(peeled(&ty_str(&f.ty)));
        }
    }
    out
}

/// Every type named by a return of a public method, in every inherent impl.
/// Drives which enums get mirrored, so the set follows the wrapped surface
/// instead of a list -- and an accessor on a generated wrapper class gets its
/// enum mirrored too.
pub fn returned_types(files: &[syn::File]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() {
            continue;
        }
        for it in &imp.items {
            if let ImplItem::Fn(f) = it
                && let (true, ReturnType::Type(_, t)) = (is_pub(&f.vis), &f.sig.output)
            {
                out.insert(ty_str(t));
            }
        }
    }
    out
}

/// Length of a `static NAME: &[T] = &[...]` array. Upstream may index such a
/// table with an option field *unchecked*, so the bound has to come from upstream
/// rather than from a magic number here.
pub fn static_array_len(files: &[syn::File], name: &str) -> usize {
    let s = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Static(s) if s.ident == name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("upstream: `static {name}` not found"));
    let expr = match &*s.expr {
        syn::Expr::Reference(r) => &*r.expr,
        other => other,
    };
    match expr {
        syn::Expr::Array(a) => a.elems.len(),
        _ => panic!("upstream: `{name}` is no longer an array literal"),
    }
}

pub fn struct_fields_opt<'a>(files: &'a [syn::File], ty: &str) -> Option<&'a Fields> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(&s.fields),
        _ => None,
    })
}

pub fn struct_fields<'a>(files: &'a [syn::File], ty: &str) -> &'a syn::FieldsNamed {
    match struct_fields_opt(files, ty) {
        Some(Fields::Named(named)) => named,
        Some(_) => panic!("upstream {ty} is no longer a struct with named fields"),
        None => panic!("upstream: `pub struct {ty}` not found"),
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

/// `toy::Shape`, or `root::module::Name` for a type a module exports.
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
}

/// `impl Deref for X { type Target = Y }` -> Y. Upstream uses it as inheritance
/// (`LinearGradient` derefs to `BaseGradient`), and a per-impl scan cannot see
/// through it, so the chain has to be walked explicitly.
pub fn deref_target(files: &[syn::File], ty: &str) -> Option<String> {
    files.iter().flat_map(|f| &f.items).find_map(|i| {
        let Item::Impl(imp) = i else { return None };
        // syn 3: `trait_` is `(Path, Token![for])`; in syn 2 the path sat at `.1`
        // behind the negative-impl bang, which has moved into `modifiers`.
        let (trait_path, _) = imp.trait_.as_ref()?;
        if !trait_path
            .segments
            .last()
            .is_some_and(|s| s.ident == "Deref")
            || !self_ident(imp).is_some_and(|id| id == ty)
        {
            return None;
        }
        imp.items.iter().find_map(|it| match it {
            ImplItem::Type(t) if t.ident == "Target" => Some(ty_str(&t.ty)),
            _ => None,
        })
    })
}

/// The upstream name inside a registry key: `filter::Image` -> `Image`.
///
/// A key carries the module so that two types of the same name stay distinct;
/// every lookup into the sources wants the name alone.
pub fn bare(key: &str) -> &str {
    key.rsplit_once("::").map(|(_, n)| n).unwrap_or(key)
}

/// The JS name of a data type, whatever its registry key.
///
/// A key can be module-qualified. A crate may define `Image` twice -- in
/// `filter.rs` and in `mod.rs` -- and the two cannot share a bare key without the path
/// resolving to one and the members to the other.
pub fn data_ident(ty: &str) -> proc_macro2::Ident {
    if let Some((module, name)) = ty.rsplit_once("::") {
        let head = module.rsplit("::").next().unwrap_or(module);
        return format_ident!("{}{name}", head.to_case(Case::Pascal));
    }
    format_ident!("{}", ty)
}

/// JS name for a mapped enum: its upstream name.
pub fn enum_ident(name: &str) -> proc_macro2::Ident {
    format_ident!("{}", name)
}

/// JS class name for an Arc-held upstream type: `filter::Filter` -> `Filter`.
pub fn wrapper_ident(ty: &str) -> proc_macro2::Ident {
    format_ident!("{}", ty.rsplit("::").next().unwrap_or(ty))
}

/// Rust path of that definition, as reachable from our crate.
pub fn wrapper_path(
    ty: &str,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> TokenStream {
    if ty.contains("::") {
        let segs: Vec<_> = ty.split("::").map(|s| format_ident!("{}", s)).collect();
        quote!(#root::#(#segs)::*)
    } else {
        upstream_path(ty, modules, root)
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

/// `render_png` -> `renderPng`, the name napi gives it in JS. An all-caps
/// name is an acronym: `ImageKind::PNG` is `png`.
pub fn lower_camel(s: &str) -> String {
    s.to_case(Case::Camel)
}
