//! Turning what the vocabulary knows into Rust that napi can expand.
//!
//! Phase 3. Every function here takes upstream shapes and a `Vocab` and returns
//! tokens: a struct, a class, a union, a method list.
//!
//! The two emitters differ only in how they package the same members.
//! `object_struct` is for a type that maps completely; `value_class` for one
//! that maps partially -- napi needs `Clone` and `FromNapiValue` of every field
//! of an object and a class has neither, so a partial mapping cannot be a field
//! of anything, and demoting one type forces every type holding it down too.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use syn::{Fields, ImplItem, Item, ReturnType};

use crate::sources::is_pub;
use crate::vocab::*;

pub struct Field {
    pub decl: TokenStream,   // the generated `pub name: Option<T>,`
    pub assign: TokenStream, // `if let Some(v) = ... { o.name = ... }`
}

pub fn field(
    ident: &syn::Ident,
    doc: &[TokenStream],
    jsty: TokenStream,
    assign: TokenStream,
) -> Field {
    Field {
        decl: quote! { #(#doc)* pub #ident: Option<#jsty>, },
        assign,
    }
}

/// One carried type as an object field: its JS type, and how to convert the
/// binding `access` into it. None when it cannot be a field at all --
/// `payload_blocker` is the authority on *whether*, this is the *how*.
pub fn carried_field(
    vocab: &Vocab,
    ty: &str,
    access: &TokenStream,
) -> Option<(Option<&'static str>, TokenStream, TokenStream)> {
    Some(match vocab.classify(ty) {
        Some(Js::Str) => (None, quote!(String), quote!(#access.to_string())),
        Some(Js::F32) | Some(Js::F64) => (None, quote!(f64), quote!(*#access as f64)),
        Some(Js::U32) | Some(Js::U8) | Some(Js::U16) => {
            (None, quote!(u32), quote!(*#access as u32))
        }
        Some(Js::I32) => (None, quote!(i32), quote!(*#access)),
        Some(Js::F32List) => (
            None,
            quote!(Vec<f64>),
            quote!(#access.iter().map(|x| *x as f64).collect()),
        ),
        Some(Js::Scalar) => (None, quote!(f64), quote!(#access.get() as f64)),
        Some(Js::Bool) => (None, quote!(bool), quote!(*#access)),
        Some(Js::Enum) => {
            let e = enum_ident(&vocab.resolve(ty));
            (None, quote!(#e), quote!(#e::from(*#access)))
        }
        Some(Js::Object(t)) => {
            let o = data_ident(&t);
            (None, quote!(#o), quote!(#o::from(#access)))
        }
        // An Arc-held definition is shared by every element referencing it, so
        // it is named rather than copied -- and the field is called `id`, because
        // it should say what it is instead of hiding behind `value`.
        Some(Js::Handle(t)) if vocab.with_id.contains(&t) => {
            (Some("id"), quote!(String), quote!(#access.id().to_string()))
        }
        _ => return None,
    })
}

/// The union for one payload enum: a struct per payload variant, one shared
/// struct for the unit ones, and the converter that produces them.
///
/// Err with a reason rather than a partial union: half a discriminated union is
/// worse than none, because the missing arm is invisible at the call site.
pub fn payload_enum_code(
    name: &str,
    info: &PayloadEnum,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
) -> std::result::Result<TokenStream, String> {
    let up = upstream_path(name, modules, &quote!(usvg));
    let mut items = TokenStream::new();
    let mut arms: Vec<TokenStream> = Vec::new();
    let mut i = 0usize;

    if let Some(unit) = info.unit_ident(name) {
        let ts = info
            .variants
            .iter()
            .filter(|(_, p)| matches!(p, Payload::None))
            .map(|(n, _)| format!("'{}'", lower_camel(n)))
            .collect::<Vec<_>>()
            .join(" | ");
        let doc = format!(" The payload-free variants of `{name}`.");
        items.extend(quote! {
            #[doc = #doc]
            #[napi(object)]
            #[derive(Clone)]
            pub struct #unit {
                #[doc = " Discriminant. Narrow on this."]
                #[napi(ts_type = #ts)]
                pub r#type: String,
            }
        });
        let arm = info.either_arm(name, 0);
        for (n, p) in info
            .variants
            .iter()
            .filter(|(_, p)| matches!(p, Payload::None))
        {
            let _ = p;
            let vid = format_ident!("{}", n);
            let tag = lower_camel(n);
            arms.push(quote!(#up::#vid => #arm(#unit { r#type: #tag.to_string() }),));
        }
        i = 1;
    }

    for (n, payload) in &info.variants {
        if matches!(payload, Payload::None) {
            continue;
        }
        let sid = info.variant_ident(name, n);
        let vid = format_ident!("{}", n);
        let tag = lower_camel(n);
        let ts = format!("'{tag}'");
        let arm = info.either_arm(name, i);
        i += 1;

        // What the struct declares, how the arm binds it, and how each field is
        // built. The three shapes differ only here.
        let (decls, pattern, inits) = match payload {
            Payload::None => unreachable!("skipped above"),
            Payload::Value(ty) if vocab.classify(ty).is_none() => {
                report!(
                    "{name}::{n} carries {ty}, which has no mappable data: emitted without a value"
                );
                (Vec::new(), quote!(#up::#vid(_)), Vec::new())
            }
            // A payload that no object field can hold, but a getter can. Only
            // bytes today: `Buffer` is a handle into the JS heap and is not
            // `Clone`, which `#[napi(object)]` requires of every field, so the
            // variant becomes a read-only class. Narrowing still works -- the
            // discriminant is a getter with a literal return type.
            Payload::Value(ty) if matches!(vocab.classify(ty), Some(Js::Bytes)) => {
                let doc = format!(" `{name}::{n}`. The bytes are the document's own.");
                let bytes_doc = " The encoded bytes, exactly as the document supplied them: \
usvg does not decode them, and neither does this.";
                // The payload's own type, not `Vec<u8>`: usvg holds these behind
                // an `Arc`, and copying out of it here would deep-copy the whole
                // encoded image before anyone asked for it. Cloning the `Arc` is
                // a refcount bump, and the one unavoidable copy happens in the
                // getter, when the JS `Buffer` is actually built.
                // Qualified: the generated file imports the napi prelude and usvg,
                // not `std::sync`, and the rest of it spells `Arc` out in full.
                let raw_src = ty.replace("Arc<", "std::sync::Arc<");
                let raw_ty: syn::Type = syn::parse_str(&raw_src)
                    .unwrap_or_else(|e| panic!("{name}::{n} carries {ty}, unparseable: {e}"));
                items.extend(quote! {
                    #[doc = #doc]
                    #[napi]
                    pub struct #sid {
                        raw: #raw_ty,
                    }
                    #[napi]
                    impl #sid {
                        // Not `fn r#type`: napi-derive builds
                        // `r#type_c_callback` from the method name and panics on
                        // it. `js_name` sets what JavaScript sees, which is all
                        // that matters.
                        #[doc = " Discriminant. Narrow on this."]
                        #[napi(getter, js_name = "type", ts_return_type = #ts)]
                        pub fn kind_tag(&self) -> &'static str {
                            #tag
                        }
                        #[doc = #bytes_doc]
                        #[napi(getter)]
                        pub fn bytes(&self) -> Buffer {
                            self.raw.as_slice().into()
                        }
                    }
                });
                arms.push(quote!(#up::#vid(v) => #arm(#sid { raw: v.clone() }),));
                continue;
            }
            Payload::Value(ty) => {
                let access = quote!(v);
                let Some((hint, fty, expr)) = carried_field(vocab, ty, &access) else {
                    return Err(format!("{name}::{n} carries {ty}, which cannot be a field"));
                };
                let field = format_ident!("{}", hint.unwrap_or("value"));
                (
                    vec![quote!(pub #field: #fty,)],
                    quote!(#up::#vid(v)),
                    vec![quote!(#field: #expr,)],
                )
            }
            Payload::Fields(fields) => {
                let mut decls = Vec::new();
                let mut binds = Vec::new();
                let mut inits = Vec::new();
                for (fname, ty) in fields {
                    let id = format_ident!("{}", fname);
                    let access = quote!(#id);
                    let Some((_, fty, expr)) = carried_field(vocab, ty, &access) else {
                        return Err(format!(
                            "{name}::{n}.{fname} carries {ty}, which cannot be a field"
                        ));
                    };
                    decls.push(quote!(pub #id: #fty,));
                    binds.push(quote!(#id));
                    inits.push(quote!(#id: #expr,));
                }
                (decls, quote!(#up::#vid { #(#binds),* }), inits)
            }
        };

        let doc = format!(" `{name}::{n}`.");
        items.extend(quote! {
            #[doc = #doc]
            #[napi(object)]
            #[derive(Clone)]
            pub struct #sid {
                #[doc = " Discriminant. Narrow on this."]
                #[napi(ts_type = #ts)]
                pub r#type: String,
                #(#decls)*
            }
        });
        arms.push(quote!(#pattern => #arm(#sid {
            r#type: #tag.to_string(),
            #(#inits)*
        }),));
    }

    let conv = info.conv_ident(name);
    let ety = info.either_ty(name);
    items.extend(quote! {
        fn #conv(v: &#up) -> #ety {
            match v { #(#arms)* }
        }
    });
    Ok(items)
}

/// Maps one flat config struct to `#[napi(object)]` fields plus the code that
/// writes them back onto the real Rust struct. Used for both `usvg::Options`
/// and `usvg::WriteOptions`.
pub fn map_struct(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    precision_max: u32,
) -> (Vec<Field>, Vec<String>, usize) {
    let named = struct_fields(files, ty);

    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut clamped = 0usize;

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
                //
                // The suffix is how the field is recognised, and a rename
                // upstream would silently fall through to u8::MAX -- which is an
                // index 242 places past the end of a 13-entry table, reached from
                // `write_num` for every coordinate. That is an out-of-bounds
                // panic unwinding out of an `extern "C"` callback, so a process
                // abort rather than a JS exception. The caller counts what this
                // matched and fails the build when it matches nothing.
                let max = if name.ends_with("_precision") {
                    clamped += 1;
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
            // Read-only: a union describes what a document holds, never what a
            // render option sets.
            | Some(Js::U16)
            | Some(Js::I32)
            | Some(Js::PayloadEnum(_))
            | Some(Js::PayloadEnumList(_))
            | Some(Js::PathData)
            | Some(Js::Tag4)
            | Some(Js::F32List)
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

    (out, skipped, clamped)
}

pub fn map_enums(
    files: &[syn::File],
    wanted: &BTreeSet<String>,
    modules: &BTreeMap<String, String>,
    // Where the upstream type lives: `usvg` for its own enums, `usvg::fontdb`
    // for the ones fontdb defines. Hardcoding `usvg` meant fontdb's enums could
    // never be mapped, which is why `FaceInfo.style` was missing.
    root: &TokenStream,
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
        let ident = enum_ident(&e.ident.to_string());
        let ident = &ident;
        let variants: Vec<_> = e.variants.iter().map(|v| v.ident.clone()).collect();
        let doc = docs(&e.attrs);
        // The upstream path is the upstream name, not the renamed one.
        let up = upstream_path(&e.ident.to_string(), modules, root);
        // The vocabulary is keyed by the upstream name: that is what a member's
        // type string says, and what `classify` is handed.
        names.insert(e.ident.to_string());
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

/// Emits `#[napi(object)] pub struct X { .. }` plus `From<&upstream>` for a plain
/// data type: accessors first, public fields if it has no accessors. Members
/// that do not map are dropped -- the object is deliberately partial.
/// One mappable member of a data type: its JS name, its JS type, and the
/// expression that produces it from a binding named `v`.
pub struct Member {
    pub id: proc_macro2::Ident,
    pub jsty: TokenStream,
    pub value: TokenStream,
}

/// The TypeScript type to force, when napi would rewrite it from the name alone.
///
/// napi-derive-backend maps a Rust type *named* `String`, `char`, `OsStr`,
/// `PathBuf`, `Path`, `BigInt`, `Symbol`, `Null` or `JsFunction` to a JS
/// primitive, reading the name and not the type. `Path` is the one that collides
/// with a type this generator emits, and the collision is a property of napi
/// rather than of this API -- so it is answered wherever a member is emitted,
/// not at the one call site that noticed it first.
///
/// `usvg::layout::Span` carries three `Option<Path>` fields, and they shipped
/// declared as `string` while holding a whole `Path` object.
pub fn napi_name_clash(jsty: &TokenStream) -> Option<(String, String)> {
    const CLASHES: [&str; 9] = [
        "String",
        "char",
        "OsStr",
        "PathBuf",
        "Path",
        "BigInt",
        "Symbol",
        "Null",
        "JsFunction",
    ];
    let t = jsty.to_string().replace(' ', "");
    // `Option<Path>` and `Path`, but not `PathSegment`, `ClipPath` or `Vec<Path>`
    // -- napi only rewrites the type itself, so only those two shapes need it.
    let (inner, optional) = match t.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
        Some(i) => (i, true),
        None => (t.as_str(), false),
    };
    if !CLASHES.contains(&inner) {
        return None;
    }
    // A field keeps its own optionality: napi derives `?` from the Rust type
    // independently of `ts_type`, so spelling the union here would give
    // `underline?: Path | null`. A getter's return type does not, so it does.
    Some((
        inner.to_string(),
        if optional {
            format!("{inner} | null")
        } else {
            inner.to_string()
        },
    ))
}

/// Walks a data type's members -- accessors first, public fields otherwise --
/// and maps the ones it can. Shared by the strict object emitter and the value
/// class emitter, which differ only in how they package the result.
pub fn data_members(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    // Whether a member may itself be a read-only class. A getter can hand one
    // back; an `#[napi(object)]` field cannot, because napi needs Clone and
    // FromNapiValue of a field and a class has neither. So the same member is
    // mappable for `value_class` and not for `object_struct`.
    nested_values: bool,
) -> (Vec<Member>, Vec<String>, BTreeSet<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut reached = BTreeSet::new();

    // Conversions, not data: `Stroke::to_tiny_skia` hands the caller a
    // tiny-skia value to draw with. Counting it as an unmapped member would
    // demote Stroke to a read-only class, and any object holding one -- `Path`
    // -- would then carry a field napi cannot round-trip.
    const NOT_DATA: [&str; 1] = ["to_tiny_skia"];

    // The tree itself. Group, Node and Tree are what a handle exists for, and
    // following one into the data registry drags every definition type behind
    // it -- `Text::flattened` and `filter::Image::root` both hand back a Group.
    // A type rule rather than the two member names, because the next one to
    // return a Group should need no edit here.
    const NOT_DATA_TYPES: [&str; 3] = NODE_TYPES;

    let mut push = |name: &str, ty_s: &str, access: TokenStream| {
        if NOT_DATA.contains(&name) {
            return;
        }
        let peeled = ty_s
            .trim_start_matches('&')
            .trim_start_matches("Box<")
            .trim_start_matches("Option<")
            .trim_start_matches('&')
            .trim_end_matches('>');
        if NOT_DATA_TYPES.contains(&bare(peeled)) {
            return;
        }
        let id = format_ident!("{}", name);
        let (jsty, value) = match vocab.classify(ty_s) {
            Some(Js::F32) | Some(Js::F64) => (quote!(f64), quote!(#access as f64)),
            Some(Js::U32) | Some(Js::U8) | Some(Js::U16) => (quote!(u32), quote!(#access as u32)),
            Some(Js::I32) => (quote!(i32), quote!(#access)),
            Some(Js::Bool) => (quote!(bool), quote!(#access)),
            Some(Js::Count) => (quote!(u32), quote!(#access as u32)),
            Some(Js::Str) => (quote!(String), quote!(#access.to_string())),
            Some(Js::Scalar) => (quote!(f64), quote!(#access.get() as f64)),
            Some(Js::IntNewtype(_)) => (quote!(u32), quote!(#access.0 as u32)),
            Some(Js::Bbox) => (quote!(BBox), quote!(BBox::from(#access))),
            Some(Js::Matrix) => (quote!(Matrix), quote!(Matrix::from(#access))),
            Some(Js::F32List) => (
                quote!(Vec<f64>),
                quote!(#access.iter().map(|v| *v as f64).collect()),
            ),
            // A value class owns its upstream value, so a nested one is cloned
            // into its `wrap`.
            Some(Js::Value(t)) if nested_values => {
                let c = data_ident(&t);
                reached.insert(t.clone());
                (quote!(#c), quote!(#c::wrap(#access.clone())))
            }
            Some(Js::ValueList(t)) if nested_values => {
                let c = data_ident(&t);
                reached.insert(t.clone());
                (
                    quote!(Vec<#c>),
                    quote!(#access.iter().cloned().map(#c::wrap).collect()),
                )
            }
            Some(Js::OptValue(t)) if nested_values => {
                let c = data_ident(&t);
                reached.insert(t.clone());
                (
                    quote!(Option<#c>),
                    quote!(#access.map(|x| #c::wrap(x.clone()))),
                )
            }
            Some(Js::PayloadEnumList(t)) => {
                let info = &vocab.payload[&t];
                let (ety, conv) = (info.either_ty(&t), info.conv_ident(&t));
                (
                    quote!(Vec<#ety>),
                    quote!(#access.iter().map(#conv).collect()),
                )
            }
            Some(Js::PayloadEnum(t)) => {
                let info = &vocab.payload[&t];
                let (ety, conv) = (info.either_ty(&t), info.conv_ident(&t));
                let arg = if ty_s.starts_with('&') {
                    quote!(#access)
                } else {
                    quote!(&#access)
                };
                (quote!(#ety), quote!(#conv(#arg)))
            }
            Some(Js::Tag4) => (
                quote!(String),
                quote!(String::from_utf8_lossy(&#access).into_owned()),
            ),
            Some(Js::PathData) => (quote!(Vec<PathSegment>), quote!(path_segments(#access))),
            Some(Js::Enum) => {
                let e = enum_ident(&vocab.resolve(ty_s));
                (quote!(#e), quote!(#e::from(#access)))
            }
            Some(Js::Object(t)) => {
                let o = data_ident(&t);
                reached.insert(t.clone());
                let arg = if ty_s.starts_with('&') {
                    quote!(#access)
                } else {
                    quote!(&#access)
                };
                (quote!(#o), quote!(#o::from(#arg)))
            }
            Some(Js::ObjectList(t)) => {
                let o = data_ident(&t);
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
                    Some((_, Js::F32List)) => {
                        out.push(Member {
                            id,
                            jsty: quote!(Option<Vec<f64>>),
                            value: quote!(
                                #access.map(|v| v.iter().map(|x| *x as f64).collect())
                            ),
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

    // Fields and accessors both, not one or the other.
    //
    // This used to take accessors when there were any and fall back to public
    // fields only when there were none, which made the choice a whole-type
    // switch: a single new `pub fn` upstream flipped a type off its fields and
    // deleted every one of them. Adding `fn is_black(&self) -> bool` to
    // `usvg::Color` replaced `red`, `green` and `blue` with `isBlack` -- no
    // report line, no dropped member, the completeness assert satisfied because
    // nothing had been dropped. It just emitted a different type.
    //
    // Accessors first so a name they share with a field resolves to the
    // accessor, which is the one upstream means to be read.
    let mut seen: BTreeSet<String> = BTreeSet::new();
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
            seen.insert(id.to_string());
            push(&id.to_string(), &ty_str(o), quote!(v.#id()));
        }
    }
    if let Some(Fields::Named(named)) = struct_fields_opt(files, ty) {
        for f in &named.named {
            if !is_pub(&f.vis) {
                continue;
            }
            let id = f.ident.clone().unwrap();
            if seen.contains(&id.to_string()) {
                continue;
            }
            push(&id.to_string(), &ty_str(&f.ty), quote!(v.#id.clone()));
        }
    }
    (out, skipped, reached)
}

/// Strict `#[napi(object)]`: emitted only when *every* member maps, so the value
/// is a faithful copy that could also be rebuilt.
pub fn object_struct(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> (TokenStream, Vec<String>, BTreeSet<String>) {
    let (members, skipped, reached) = data_members(files, bare(ty), vocab, false);
    if members.is_empty() || !skipped.is_empty() {
        return (TokenStream::new(), skipped, reached);
    }
    let name = format_ident!("{}", ty);
    let up = upstream_path(ty, modules, root);
    let decls = members.iter().map(|m| {
        let (id, t) = (&m.id, &m.jsty);
        match napi_name_clash(t) {
            Some((field_ty, _)) => quote!(#[napi(ts_type = #field_ty)] pub #id: #t,),
            None => quote!(pub #id: #t,),
        }
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
pub fn value_class(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> (TokenStream, Vec<String>, BTreeSet<String>) {
    let (members, skipped, reached) = data_members(files, bare(ty), vocab, true);
    if members.is_empty() {
        return (TokenStream::new(), skipped, reached);
    }
    let name = data_ident(ty);
    let up = upstream_path(ty, modules, root);
    let getters = members.iter().map(|m| {
        let (id, t, e) = (&m.id, &m.jsty, &m.value);
        let attr = match napi_name_clash(t) {
            Some((_, ret_ty)) => quote!(#[napi(getter, ts_return_type = #ret_ty)]),
            None => quote!(#[napi(getter)]),
        };
        quote! {
            #attr
            pub fn #id(&self) -> #t {
                let v = &self.inner;
                #e
            }
        }
    });
    // `a Image` reads wrong in a published declaration, and these names come
    // from upstream so the vowels are not ours to choose.
    let article = if ty.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    let doc = format!(" Read-only view of {article} `{ty}`.");
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

/// Emits `#[napi] pub struct T { inner: Arc<...> }` plus its read-only
/// accessors, and reports the handle types those accessors reach in turn.
pub fn wrapper_class(
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
pub struct MethodPass<'a> {
    pub files: &'a [syn::File],
    pub ty: &'a str,
    pub receiver: &'a TokenStream,
    pub skip: &'a [&'a str],
    /// Emitted before the call; when set, every wrapper returns `Result<_>` so a
    /// receiver that has to be looked up first can fail cleanly.
    pub prologue: Option<&'a TokenStream>,
    /// An `Arc` receiver cannot hand out `&mut`, so drop mutating methods.
    pub readonly: bool,
}

pub fn map_methods(
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
                    let o = data_ident(t);
                    (quote!(#o), quote!(#o::from(#call)), false)
                }
                Ret::ObjectList(ref t) => {
                    let o = data_ident(t);
                    (
                        quote!(Vec<#o>),
                        quote!(#call.into_iter().map(#o::from).collect()),
                        false,
                    )
                }
                Ret::OptObject(ref t) => {
                    let o = data_ident(t);
                    (
                        quote!(Option<#o>),
                        quote!(#call.map(|x| #o::from(x))),
                        false,
                    )
                }
                Ret::OptValue(ref t) => {
                    let w = data_ident(t);
                    (
                        quote!(Option<#w>),
                        quote!(#call.map(|x| #w::wrap(x.clone()))),
                        false,
                    )
                }
                Ret::Value(ref t) => {
                    let w = data_ident(t);
                    (quote!(#w), quote!(#w::wrap(#call.clone())), false)
                }
                Ret::ValueList(ref t) => {
                    let w = data_ident(t);
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
pub struct Twin {
    pub target: syn::Type,
    pub core: syn::Ident,
    /// JS name of the sync method that calls this core, for the doc comment.
    pub sibling: Option<String>,
    pub public: syn::Ident,
    pub js: syn::Ident,
    pub params: Vec<(syn::Ident, syn::Type)>,
    pub output: syn::Type,
}

/// Collects the marked cores and *removes* the marker, which is not a real
/// attribute: rustc would reject it. Reading the signature is enough to write
/// the twin, so nothing about it is declared twice.
pub fn async_twins(file: &mut syn::File) -> Vec<Twin> {
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

/// The ceremony: a `Task` type per core, the public `#[napi]` method that
/// schedules it, and the `AbortSignal` plumbing. Written once, applied by rule.
pub fn emit_twins(twins: &[Twin]) -> TokenStream {
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

/// Method names the emitter template defines itself, per target type, read off
/// the template rather than listed by hand: `impl Resvg { pub fn children }`
/// means the `children` upstream method is covered, and a rule can say so.
pub fn template_fns(code: &TokenStream) -> BTreeMap<String, Vec<String>> {
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

/// Types the template hands out, peeled of the wrappers they arrive in.
///
/// `SvgNode::text` returning `Option<Text>` is the only statement anywhere that
/// `Text` should be mapped: the passes never see it, because a node payload is
/// not a collection and only collection elements become candidates. Read off
/// the template for the same reason `template_fns` is -- so that adding an
/// accessor is the whole change, with no list to update beside it.
pub fn template_returns(code: &TokenStream) -> BTreeSet<String> {
    let file: syn::File =
        syn::parse2(code.clone()).expect("the emitter template is not valid Rust on its own");
    let mut out = BTreeSet::new();
    for item in &file.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() {
            continue; // trait impls are plumbing, not API
        }
        for it in &imp.items {
            if let ImplItem::Fn(f) = it {
                // Private helpers are not API: `SvgNode::node` returning
                // `&usvg::Node` must not make Node a data candidate.
                if !is_pub(&f.vis) {
                    continue;
                }
                let ReturnType::Type(_, t) = &f.sig.output else {
                    continue;
                };
                let mut name = ty_str(t);
                // Result<Option<Text>> -> Text, in any nesting order.
                loop {
                    let before = name.clone();
                    name = name.trim_start_matches('&').to_string();
                    for w in ["Result<", "Option<", "Vec<"] {
                        if let Some(inner) = name.strip_prefix(w).and_then(|r| r.strip_suffix('>'))
                        {
                            name = inner.to_string();
                        }
                    }
                    if name == before {
                        break;
                    }
                }
                out.insert(name);
            }
        }
    }
    out
}
