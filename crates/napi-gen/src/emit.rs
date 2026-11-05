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

use convert_case::Casing;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use syn::{Fields, ImplItem, Item, ReturnType};

use crate::config;
use crate::sources::{derives, has_default, inherent_impls, is_pub, self_ident};
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

/// Where a converted value lands. The mapping from a `Js` kind to a JS type
/// and the expression producing it is one table (`js_conv`); the sites differ
/// only where they must, and each difference is an arm below rather than a
/// second copy of the table.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// A wrapped method's return value: `access` is the call.
    Method,
    /// A read-only class getter, which may hand back another class.
    Getter,
    /// An `#[napi(object)]` field, which may not.
    Field,
    /// A payload carried by a union variant, bound by reference.
    Carried,
}

/// One conversion: the JS type, the expression, the data type it reaches (for
/// the discovery fixpoint), whether the expression is already a `Result`, and
/// the field name a carried handle takes instead of `value`.
pub struct Conv {
    pub ty: TokenStream,
    pub expr: TokenStream,
    pub reached: Option<String>,
    pub is_result: bool,
    pub field: Option<&'static str>,
}

/// The one Js -> JS table. `access` is the Rust expression to convert; `src_ty`
/// its type text, for the `&` it may already carry. None: no form at this site.
pub fn js_conv(
    js: &Js,
    src_ty: &str,
    vocab: &Vocab,
    access: &TokenStream,
    site: Site,
) -> Option<Conv> {
    use Site::*;
    let a = access;
    // A carried payload is bound by reference: Copy kinds are read through it.
    let d = if site == Carried {
        quote!(*#a)
    } else {
        quote!(#a)
    };
    let plain = |ty: TokenStream, expr: TokenStream| Conv {
        ty,
        expr,
        reached: None,
        is_result: false,
        field: None,
    };
    let reaching = |ty: TokenStream, expr: TokenStream, t: &str| Conv {
        reached: Some(t.to_string()),
        ..plain(ty, expr)
    };
    // `&x` unless the member already is a reference.
    let by_ref = if src_ty.starts_with('&') {
        quote!(#a)
    } else {
        quote!(&#a)
    };
    let saturating = quote!(u32::try_from(#a).unwrap_or(u32::MAX));
    // The template's own names for the domain shapes.
    let shapes = &config().shapes;
    let [rect, size, matrix, segment, segments] = [
        shapes.rect,
        shapes.size,
        shapes.matrix,
        shapes.path.0,
        shapes.path.1,
    ]
    .map(|n| format_ident!("{n}"));
    Some(match (js, site) {
        (Js::Bool, _) => plain(quote!(bool), d),
        (Js::I32, _) => plain(quote!(i32), d),
        (Js::F32, _) => plain(quote!(f64), quote!(f64::from(#d))),
        // A method's integers saturate; a member's widen, which cannot fail.
        (Js::U8 | Js::U16 | Js::U32, Method) => plain(quote!(u32), saturating),
        (Js::U8 | Js::U16, _) => plain(quote!(u32), quote!(u32::from(#d))),
        (Js::U32, _) => plain(quote!(u32), d),
        (Js::Count, Carried) => return None,
        (Js::Count, _) => plain(quote!(u32), saturating),
        (Js::Str, _) => plain(quote!(String), quote!(#a.to_string())),
        (Js::Scalar, _) => plain(quote!(f64), quote!(f64::from(#a.get()))),
        (Js::IntNewtype(_), Carried) => return None,
        (Js::IntNewtype(_), _) => plain(quote!(u32), quote!(u32::from(#a.0))),
        (Js::Bbox, Carried) => return None,
        (Js::Bbox, _) => plain(quote!(#rect), quote!(#rect::from(#a))),
        (Js::OptBbox, Method) => plain(quote!(Option<#rect>), quote!(#a.map(#rect::from))),
        (Js::Size, Carried) => return None,
        (Js::Size, _) => plain(quote!(#size), quote!(#size::from(#a))),
        (Js::Matrix, Carried) => return None,
        (Js::Matrix, _) => plain(quote!(#matrix), quote!(#matrix::from(#a))),
        (Js::F32List, Getter | Field) => plain(
            quote!(Vec<f64>),
            quote!(#a.iter().map(|v| f64::from(*v)).collect()),
        ),
        (Js::F32List, Carried) => plain(
            quote!(Vec<f64>),
            quote!(#a.iter().map(|x| f64::from(*x)).collect()),
        ),
        (Js::Enum, _) => {
            let e = enum_ident(&vocab.resolve(src_ty));
            plain(quote!(#e), quote!(#e::from(#d)))
        }
        // A value class owns its upstream value, so a nested one is cloned
        // into its `wrap`.
        (Js::Value(t), Getter | Method) => {
            let c = data_ident(t);
            reaching(quote!(#c), quote!(#c::wrap(#a.clone())), t)
        }
        (Js::ValueList(t), Getter) => {
            let c = data_ident(t);
            let e = quote!(#a.iter().map(|x| #c::wrap(x.clone())).collect());
            reaching(quote!(Vec<#c>), e, t)
        }
        // `iter` for a slice, `into_iter` for an iterator of references.
        (Js::ValueList(t), Method) => {
            let c = data_ident(t);
            let iter = if src_ty.starts_with("&[") {
                quote!(iter)
            } else {
                quote!(into_iter)
            };
            let e = quote!(#a.#iter().map(|x| #c::wrap(x.clone())).collect());
            reaching(quote!(Vec<#c>), e, t)
        }
        (Js::OptValue(t), Getter | Method) => {
            let c = data_ident(t);
            reaching(
                quote!(Option<#c>),
                quote!(#a.map(|x| #c::wrap(x.clone()))),
                t,
            )
        }
        (Js::PayloadEnumList(t), Getter | Field) => {
            let info = &vocab.payload[t];
            let (ety, conv) = (info.either_ty(t), info.conv_ident(t));
            plain(quote!(Vec<#ety>), quote!(#a.iter().map(#conv).collect()))
        }
        (Js::PayloadEnum(t), Getter | Field) => {
            let info = &vocab.payload[t];
            let (ety, conv) = (info.either_ty(t), info.conv_ident(t));
            plain(quote!(#ety), quote!(#conv(#by_ref)))
        }
        (Js::Tag4, Getter | Field) => plain(
            quote!(String),
            quote!(String::from_utf8_lossy(&#a).into_owned()),
        ),
        (Js::PathData, Getter | Field) => plain(quote!(Vec<#segment>), quote!(#segments(#a))),
        (Js::Object(t), Getter | Field) => {
            let o = data_ident(t);
            reaching(quote!(#o), quote!(#o::from(#by_ref)), t)
        }
        (Js::Object(t), Method | Carried) => {
            let o = data_ident(t);
            reaching(quote!(#o), quote!(#o::from(#a)), t)
        }
        (Js::ObjectList(t), Getter | Field | Method) => {
            let o = data_ident(t);
            reaching(
                quote!(Vec<#o>),
                quote!(#a.iter().map(#o::from).collect()),
                t,
            )
        }
        (Js::OptObject(t), Method) => {
            let o = data_ident(t);
            reaching(quote!(Option<#o>), quote!(#a.map(|x| #o::from(x))), t)
        }
        (Js::Handle(t), Method) => {
            let w = wrapper_ident(t);
            plain(quote!(#w), quote!(#w::wrap(#a.clone())))
        }
        (Js::HandleList(t), Method) => {
            let w = wrapper_ident(t);
            plain(
                quote!(Vec<#w>),
                quote!(#a.iter().map(|x| #w::wrap(x.clone())).collect()),
            )
        }
        // An Arc-held definition is shared by every element referencing it, so
        // it is named rather than copied -- and the field is called `id`,
        // because it should say what it is instead of hiding behind `value`.
        (Js::Handle(t), Carried) if vocab.with_id.contains(t) => Conv {
            field: Some("id"),
            ..plain(quote!(String), quote!(#a.id().to_string()))
        },
        (Js::TryUnit, Method) => Conv {
            is_result: true,
            ..plain(
                quote!(()),
                quote!(#a.map_err(|e| Error::from_reason(e.to_string()))),
            )
        },
        _ => return None,
    })
}

/// One carried type as an object field: its JS type, and how to convert the
/// binding `access` into it. None when it cannot be a field at all --
/// `payload_blocker` is the authority on *whether*, this is the *how*.
pub fn carried_field(
    vocab: &Vocab,
    ty: &str,
    access: &TokenStream,
) -> Option<(Option<&'static str>, TokenStream, TokenStream)> {
    let c = js_conv(&vocab.classify(ty)?, ty, vocab, access, Site::Carried)?;
    Some((c.field, c.ty, c.expr))
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
    // The crate the enum came from, recorded when it was found rather than
    // assumed here: one crate may define every payload enum today, which is the
    // kind of fact that stops being true without anything saying so.
    let root: TokenStream = info.root.parse().expect("a crate path");
    let up = upstream_path(name, modules, &root);
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
        for (n, _) in info
            .variants
            .iter()
            .filter(|(_, p)| matches!(p, Payload::None))
        {
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
                let bytes_doc = format!(
                    " The encoded bytes, exactly as the document supplied them: \
{} does not decode them, and neither does this.",
                    info.root
                );
                // The payload's own type, not `Vec<u8>`: upstream may hold these
                // behind an `Arc`, and copying out of it here would deep-copy the
                // whole encoded payload before anyone asked for it. Cloning the
                // `Arc` is a refcount bump, and the one unavoidable copy happens
                // in the getter, when the JS `Buffer` is actually built.
                // Qualified: the generated file imports the napi prelude and the
                // wrapped crates, not `std::sync`, and the rest of it spells
                // `Arc` out in full.
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

/// `Config::index_bounded`, each suffix with the highest index its table
/// allows, read off the upstream `static` in `files`. A table that is gone
/// fails the build.
pub fn index_bounds(files: &[syn::File]) -> Vec<(&'static str, u32)> {
    config()
        .index_bounded
        .iter()
        .map(|(suffix, table)| {
            // N entries: the highest usable index is N - 1.
            let max = u32::try_from(static_array_len(files, table) - 1)
                .expect("an index-bounded table that fits u32");
            (*suffix, max)
        })
        .collect()
}

/// Maps one flat config struct to `#[napi(object)]` fields plus the code that
/// writes them back onto the real Rust struct. Used for every option struct
/// the binding maps, such as the toy crate's `Options`. `bounds` is
/// [`index_bounds`]; the count returned is how many fields it matched.
pub fn map_struct(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    bounds: &[(&str, u32)],
) -> (Vec<Field>, Vec<Skip>, usize) {
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

        if let Some(m) = map_field(f, vocab, bounds, &mut clamped) {
            out.push(m);
            continue;
        }

        // --- not transposable as data. These are wired by the emitter
        // template instead (opaque class, resolver hooks, xmlwriter's
        // `Indent`); the rest is dropped.
        if config().hand_wired_fields.contains(&name.as_str()) {
            skipped.push(Skip::new(&name, Some(&ty), Reason::HandWired));
        } else {
            skipped.push(Skip::new(&name, Some(&ty), Reason::Dropped));
        }
    }

    (out, skipped, clamped)
}

/// What [`option_object`] makes of an upstream option struct.
pub struct OptionObject {
    /// The `#[napi(object, object_to_js = false)]` struct and its
    /// `to_upstream`, named as upstream for [`namespaced`] to prefix.
    pub code: TokenStream,
    /// Fields left out, with the reason: they keep the `Default` value.
    pub skipped: Vec<Skip>,
    /// How many fields the index bounds matched.
    pub bounded: usize,
}

/// An upstream option struct with `Default`, as an input-only plain object
/// of the namespaced surface (ADR 0001, "Option structs are plain objects").
///
/// Every field is optional. `to_upstream(&self) -> Result<Upstream, String>` starts
/// from `Default::default()` and overrides only what JS provided, so a field
/// left out keeps upstream's default, whatever it is. A field maps as a
/// [`map_struct`] field does, or through `Config::providers` (a class
/// instance supplies it) or `Config::converters` (a template function
/// converts it); one with none of these is left out and keeps its default.
///
/// `None` when the struct has no `Default`, or a type parameter: a struct
/// without `Default` becomes a class with a constructor instead (ADR 0001),
/// which this does not emit. Lifetime parameters become `'static`.
pub fn option_object(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
    bounds: &[(&str, u32)],
) -> Option<OptionObject> {
    // A listed struct that is gone is upstream moving, not a struct without
    // `Default`: loud, like `struct_fields`.
    let st = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("upstream: option object `pub struct {ty}` not found"));
    if !has_default(files, ty) || st.generics.type_params().next().is_some() {
        return None;
    }
    let lifetimes = st.generics.lifetimes().map(|_| quote!('static));
    let upstream = upstream_path(ty, modules, root);
    let upstream = quote!(#upstream<#(#lifetimes),*>);
    let name = format_ident!("{ty}");
    let struct_doc = docs(&st.attrs);

    let mut decls = Vec::new();
    let mut assigns = Vec::new();
    let mut skipped = Vec::new();
    let mut bounded = 0usize;
    // A provided field holds the instance for the call: `ClassInstance`
    // borrows it from the JS object, and that needs a lifetime.
    let mut borrows = false;
    let Fields::Named(named) = &st.fields else {
        return None;
    };
    for f in &named.named {
        if !is_pub(&f.vis) {
            continue;
        }
        let ident = f.ident.clone().unwrap();
        let name = ident.to_string();
        let fty = ty_str(&f.ty);
        let doc = docs(&f.attrs);
        if let Some(p) = config().providers.iter().find(|p| p.ty == fty) {
            let class = format_ident!("{}", p.class);
            let provide: syn::Expr =
                syn::parse_str(p.provide).expect("a Config::providers expression");
            borrows = true;
            let extra: Vec<TokenStream> = if p.doc.is_empty() {
                Vec::new()
            } else {
                std::iter::once(String::new())
                    .chain(p.doc.lines().map(|l| format!(" {l}")))
                    .map(|l| quote!(#[doc = #l]))
                    .collect()
            };
            decls.push(
                quote! { #(#doc)* #(#extra)* pub #ident: Option<ClassInstance<'a, #class>>, },
            );
            assigns.push(
                quote! { if let Some(v) = &self.#ident { let v = &**v; o.#ident = #provide; } },
            );
            continue;
        }
        if let Some(c) = config().converters.iter().find(|c| c.ty == fty) {
            let js: syn::Type = syn::parse_str(c.js).expect("a Config::converters type");
            let convert = format_ident!("{}", c.convert);
            let (ts, label) = (c.ts, lower_camel(&name));
            decls.push(quote! { #(#doc)* #[napi(ts_type = #ts)] pub #ident: Option<#js>, });
            assigns.push(
                quote! { if let Some(v) = &self.#ident { o.#ident = #convert(v, #label).map_err(napi_support::coded)?; } },
            );
            continue;
        }
        match map_field(f, vocab, bounds, &mut bounded) {
            Some(m) => {
                decls.push(m.decl);
                assigns.push(m.assign);
            }
            None => skipped.push(Skip::new(&name, Some(&fty), Reason::Unprovided)),
        }
    }
    let (generics, anon) = if borrows {
        (quote!(<'a>), quote!(<'_>))
    } else {
        (TokenStream::new(), TokenStream::new())
    };
    let code = quote! {
        #(#struct_doc)*
        #[doc = ""]
        #[doc = " Every field is optional: one left out keeps the `Default` value."]
        #[napi(object, object_to_js = false)]
        #[derive(Default)]
        pub struct #name #generics {
            #(#decls)*
        }

        impl #name #anon {
            #[doc = " The upstream value: `Default`, with what JS provided on top."]
            fn to_upstream(&self) -> Result<#upstream, String> {
                let mut o = <#upstream>::default();
                #(#assigns)*
                Ok(o)
            }
        }
    };
    Some(OptionObject {
        code,
        skipped,
        bounded,
    })
}

/// One option field as a [`map_struct`] field, or `None` when its type has
/// no JS form here. Every field becomes optional so JS can pass a partial
/// object, and is written back only when JS provided it.
fn map_field(
    f: &syn::Field,
    vocab: &Vocab,
    bounds: &[(&str, u32)],
    clamped: &mut usize,
) -> Option<Field> {
    let ident = f.ident.clone()?;
    let name = ident.to_string();
    let ty = ty_str(&f.ty);
    let doc = docs(&f.attrs);
    match vocab.classify(&ty) {
            Some(Js::F32) => Some(field(
                &ident,
                &doc,
                quote!(f64),
                quote! { if let Some(v) = self.#ident { o.#ident = to_f32(v); } },
            )),
            Some(Js::U8) => {
                // A `Config::index_bounded` field indexes an upstream table
                // without a bounds check, so clamp it to that table's last
                // index. Any other u8 just saturates instead of wrapping.
                //
                // The suffix is how the field is recognised, and a rename
                // upstream would silently fall through to u8::MAX -- which is an
                // index 242 places past the end of a 13-entry table, reached from
                // `write_num` for every coordinate. That is an out-of-bounds
                // panic unwinding out of an `extern "C"` callback, so a process
                // abort rather than a JS exception. The caller counts what this
                // matched and fails the build when it matches nothing.
                let max = match bounds.iter().find(|(suffix, _)| name.ends_with(suffix)) {
                    Some((_, max)) => {
                        *clamped += 1;
                        *max
                    }
                    None => u32::from(u8::MAX),
                };
                Some(field(
                    &ident,
                    &doc,
                    quote!(u32),
                    quote! { if let Some(v) = self.#ident { o.#ident = u8::try_from(v.min(#max)).unwrap_or(u8::MAX); } },
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
                quote! { if let Some(v) = &self.#ident { o.#ident = Some(v.clone()); } },
            )),
            Some(Js::OptPath) => Some(field(
                &ident,
                &doc,
                quote!(String),
                quote! { if let Some(v) = &self.#ident { o.#ident = Some(std::path::PathBuf::from(v)); } },
            )),
            Some(Js::StrList) => Some(field(
                &ident,
                &doc,
                quote!(Vec<String>),
                quote! { if let Some(v) = &self.#ident { o.#ident = v.clone(); } },
            )),
            Some(Js::Enum) => {
                // The JS name, after `Config::enum_renames`, as every other site spells it.
                let e = enum_ident(&vocab.resolve(&ty));
                Some(field(
                    &ident,
                    &doc,
                    quote!(#e),
                    quote! { if let Some(v) = self.#ident { o.#ident = v.into(); } },
                ))
            }
            // Size is opaque with a fallible constructor: flatten to a pair.
            Some(Js::Size) => {
                let size_new: syn::Path =
                    syn::parse_str(config().size_new).expect("a Config::size_new path");
                let w = format_ident!("{}_width", name);
                let h = format_ident!("{}_height", name);
                Some(Field {
                    decl: quote! {
                        #(#doc)* pub #w: Option<f64>,
                        #(#doc)* pub #h: Option<f64>,
                    },
                    assign: quote! {
                        if let (Some(w), Some(h)) = (self.#w, self.#h)
                            && let Some(s) = #size_new(to_f32(w), to_f32(h))
                        {
                            o.#ident = s;
                        }
                    },
                })
            }
            // Class instances and nested objects belong on methods, not in a
            // flat config object.
            Some(Js::Bytes)
            | Some(Js::Count)
            | Some(Js::TryUnit)
            | Some(Js::Matrix)
            // Unreached by any upstream option field today.
            | Some(Js::U32)
            | Some(Js::Bbox)
            | Some(Js::OptBbox)
            | Some(Js::Scalar)
            | Some(Js::IntNewtype(_))
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
    }
}

pub fn map_enums(
    files: &[syn::File],
    wanted: &BTreeSet<String>,
    modules: &BTreeMap<String, String>,
    // Where the upstream type lives: the crate's own path for its enums, the
    // re-export path for a crate reached through another (`a::b`). Hardcoding
    // the first crate's path meant the second's enums could never be mapped.
    root: &TokenStream,
) -> (BTreeSet<String>, TokenStream, Vec<(String, String)>) {
    let mut names = BTreeSet::new();
    let mut code = TokenStream::new();
    // Wanted enums that cannot be emitted, and why: neither a plain enum nor
    // a payload one. Skipped without a word, they read as never wanted.
    let mut rejected = Vec::new();

    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Enum(e) = item else { continue };
        if !is_pub(&e.vis) || !wanted.contains(&e.ident.to_string()) {
            continue;
        }
        // A payload enum is mapped elsewhere, as a union, when its shape
        // allows; when it does not, this is the one place that says so.
        if let Err(why) = payload_shape(e, &|t| t) {
            rejected.push((e.ident.to_string(), why));
            continue;
        }
        if !e.variants.iter().all(|v| matches!(v.fields, Fields::Unit)) {
            continue; // enum with payload -> mapped as a union, if at all
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

    (names, code, rejected)
}

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
/// A struct with an `Option<Path>` field, `Path` being an upstream type, ships
/// declared as `string` while holding a whole `Path` object.
///
/// Only the names an upstream crate could define for itself are listed. A
/// member typed `String`, `char`, `OsStr` or `PathBuf` *is* the std type, and
/// napi's `string` is right for it: forcing the name there declared `String`,
/// the wrapper object type, which a `string` is not assignable to.
pub fn napi_name_clash(jsty: &TokenStream) -> Option<(String, String)> {
    const CLASHES: [&str; 5] = ["Path", "BigInt", "Symbol", "Null", "JsFunction"];
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
) -> (Vec<Member>, Vec<Skip>, BTreeSet<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut reached = BTreeSet::new();

    // `Config::not_data`: conversions such as `Stroke::to_tiny_skia`.

    // The tree itself. Group, Node and Tree are what a handle exists for, and
    // following one into the data registry drags every definition type behind
    // it -- `Text::flattened` and `filter::Image::root` both hand back a Group.
    // A type rule rather than the two member names, because the next one to
    // return a Group should need no edit here: `Config::node_types`.

    let mut push = |name: &str, ty_s: &str, access: TokenStream| {
        if config().not_data.contains(&name) {
            return;
        }
        let peeled = peel_type(ty_s);
        if config().node_types.contains(&bare(peeled)) {
            return;
        }
        let id = format_ident!("{}", name);
        let js = vocab.classify(ty_s);
        // `Image` was a read-only class only because `size` did not map;
        // mapping it promoted Image to an object holding `ImageKind`, whose
        // byte variants are classes. Say so instead of relying on luck.
        if let Some(Js::PayloadEnum(t) | Js::PayloadEnumList(t)) = &js
            && !nested_values
            && vocab.payload[t].holds_class(vocab)
        {
            skipped.push(Skip::new(name, Some(ty_s), Reason::UnionWithClasses));
            return;
        }
        let site = if nested_values {
            Site::Getter
        } else {
            Site::Field
        };
        if let Some(c) = js
            .as_ref()
            .and_then(|j| js_conv(j, ty_s, vocab, &access, site))
        {
            reached.extend(c.reached);
            out.push(Member {
                id,
                jsty: c.ty,
                value: c.expr,
            });
            return;
        }
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
            Some((_, Js::F32)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<f64>),
                    value: quote!(#access.map(f64::from)),
                });
            }
            Some((_, Js::Scalar)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<f64>),
                    value: quote!(#access.map(|x| f64::from(x.get()))),
                });
            }
            Some((_, Js::F32List)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<Vec<f64>>),
                    value: quote!(
                        #access.map(|v| v.iter().map(|x| f64::from(*x)).collect())
                    ),
                });
            }
            Some((_, Js::Object(t))) => {
                let o = data_ident(&t);
                reached.insert(t);
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
            }
            _ => {
                skipped.push(Skip::new(name, Some(ty_s), Reason::Member));
            }
        }
    };

    // Fields and accessors both, not one or the other.
    //
    // This used to take accessors when there were any and fall back to public
    // fields only when there were none, which made the choice a whole-type
    // switch: a single new `pub fn` upstream flipped a type off its fields and
    // deleted every one of them. Adding `fn is_black(&self) -> bool` to a
    // `Color` replaced `red`, `green` and `blue` with `isBlack` -- no
    // report line, no dropped member, the completeness assert satisfied because
    // nothing had been dropped. It just emitted a different type.
    //
    // Accessors first so a name they share with a field resolves to the
    // accessor, which is the one upstream means to be read.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for imp in inherent_impls(files, ty) {
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
            // Copy kinds are read by value. Should upstream make one of them
            // non-Copy, this stops compiling rather than silently cloning.
            let ty_s = ty_str(&f.ty);
            let inner = ty_s
                .strip_prefix("Option<")
                .and_then(|t| t.strip_suffix('>'))
                .unwrap_or(&ty_s);
            let copy = matches!(
                vocab.classify(inner),
                Some(
                    Js::F32
                        | Js::U8
                        | Js::U16
                        | Js::U32
                        | Js::I32
                        | Js::Bool
                        | Js::Enum
                        | Js::Scalar
                        | Js::IntNewtype(_)
                )
            );
            let access = if copy {
                quote!(v.#id)
            } else {
                quote!(v.#id.clone())
            };
            push(&id.to_string(), &ty_s, access);
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
) -> (TokenStream, Vec<Skip>, BTreeSet<String>) {
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
) -> (TokenStream, Vec<Skip>, BTreeSet<String>) {
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

/// Emits `#[napi] pub struct T { inner: Arc<...> }` plus its accessors, and
/// reports the handle types those accessors reach in turn. A handle whose
/// upstream type derives `Clone` gets its `&mut self` methods through
/// `Arc::make_mut`, copy on write; the rest are read-only.
pub fn wrapper_class(
    files: &[syn::File],
    ty: &str,
    vocab: &Vocab,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
) -> (TokenStream, Vec<Skip>, BTreeSet<String>) {
    let name = wrapper_ident(ty);
    let path = wrapper_path(ty, modules, root);
    // An `Arc` hands out `&mut` only through `make_mut`, which clones a value
    // shared with anyone else first: the rule needs `Clone`, and takes it
    // from the derive.
    let make_mut = derives(files, bare(ty), "Clone");
    let inner_mut = quote!(std::sync::Arc::make_mut(&mut self.inner));
    // Walk the type itself, then whatever it derefs to, sharing one name set so
    // an override on the outer type wins.
    let mut taken = BTreeSet::new();
    let mut methods = TokenStream::new();
    let mut skipped = Vec::new();
    let mut current = Some(ty.rsplit("::").next().unwrap_or(ty).to_string());
    let inner = quote!(self.inner);
    for depth in 0.. {
        let Some(t) = current.take() else { break };
        // A chain this long is upstream changing shape, not a deeper class:
        // cutting it short dropped the remaining targets' methods silently.
        assert!(depth < 4, "{ty}: Deref chain longer than 4, reached {t}");
        // `make_mut` gives `&mut` to the type itself, not to what it derefs
        // to: a Deref target's mutating methods stay out.
        let mutable = make_mut && depth == 0;
        let (code, sk) = map_methods(
            &MethodPass {
                files,
                ty: &t,
                receiver: &inner,
                receiver_mut: mutable.then_some(&inner_mut),
                skip: &[],
                readonly: !mutable,
                standin: false,
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
            let t = s.ty.as_deref().filter(|_| s.reason == Reason::Returns)?;
            match vocab.classify(t) {
                Some(Js::Handle(x)) | Some(Js::HandleList(x)) => Some(x),
                _ => None,
            }
        })
        .collect();
    if methods.is_empty() {
        return (TokenStream::new(), skipped, reached);
    }
    // The crate path as the generated file spells it, not as quote! does.
    let crate_path = root.to_string().replace(' ', "");
    let doc = if make_mut {
        format!(" Handle on a `{crate_path}::{ty}`, copied on the first write while shared.")
    } else {
        format!(" Read-only handle on a `{crate_path}::{ty}`.")
    };
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
    };
    (code, skipped, reached)
}

/// What one mapping pass reads: which sources, which upstream type, how the
/// wrapper reaches its receiver, and what to leave alone. A struct because
/// `config::PASSES` already carries these, and unpacking it into positional
/// arguments was how this grew to eight of them.
pub struct MethodPass<'a> {
    pub files: &'a [syn::File],
    pub ty: &'a str,
    pub receiver: &'a TokenStream,
    /// What a `&mut self` method is called on, when it differs: a
    /// copy-on-write `Arc` hands out `&mut` through `make_mut`.
    pub receiver_mut: Option<&'a TokenStream>,
    pub skip: &'a [&'a str],
    /// An `Arc` receiver cannot hand out `&mut`, so drop mutating methods.
    pub readonly: bool,
    /// Whether `Config::arg_standin` applies: its class is the template's,
    /// which a namespaced class must not take.
    pub standin: bool,
}

/// Wraps every `pub fn` of `impl <ty>` whose whole signature is transposable.
/// `receiver` is the expression the call hangs off, so the same pass serves
/// a handle's `self.inner` or a borrow such as `self.inner.root()`.
pub fn map_methods(
    m: &MethodPass,
    vocab: &Vocab,
    // Method names already emitted on the target class: two passes can land on
    // the same `impl` block (`Tree::filters` and `Group::filters` both do).
    taken: &mut BTreeSet<String>,
) -> (TokenStream, Vec<Skip>) {
    // Destructured by name so the body below is unchanged from when these were
    // eight parameters.
    let MethodPass {
        files,
        ty,
        receiver,
        receiver_mut,
        skip,
        readonly,
        standin: with_standin,
    } = *m;
    let mut code = TokenStream::new();
    let mut skipped = Vec::new();
    let mut twins = BTreeSet::new();
    // `Config::arg_standin`: the class JS hands over, and its resolver.
    let standin_cfg = config().arg_standin.filter(|_| with_standin);
    let standin = standin_cfg
        .map(|(_, class, resolve)| (format_ident!("{class}"), format_ident!("{resolve}")));

    for imp in inherent_impls(files, ty) {
        for it in &imp.items {
            let ImplItem::Fn(f) = it else { continue };
            if !is_pub(&f.vis) {
                continue;
            }
            let name = f.sig.ident.to_string();
            if skip.contains(&name.as_str()) {
                continue; // renderer internals, no meaning on the JS side
            }
            // Already emitted on this class by an earlier pass, or a cfg twin
            // of one seen in this pass. A name is only reserved once its
            // method is emitted (below): reserving it on sight let a method
            // skipped here hide a mappable one of the same name on a later
            // pass or Deref target, with nothing in the report.
            if taken.contains(&name) || !twins.insert(name.clone()) {
                continue;
            }
            let gate = method_gate(imp, f);

            // generic param -> bound text, so `P: AsRef<std::path::Path>` can
            // be classified.
            let bounds: BTreeMap<String, String> = f
                .sig
                .generics
                .type_params()
                .map(|p| (p.ident.to_string(), quote!(#p).to_string().replace(' ', "")))
                .collect();

            let mut inputs = f.sig.inputs.iter();
            let Some(syn::FnArg::Receiver(recv)) = inputs.next() else {
                skipped.push(Skip::new(&name, None, Reason::NoReceiver));
                continue;
            };
            // syn 3 moved the `&mut self` marker into `ReceiverKind::Reference`;
            // `Receiver::mutability` now only covers by-value `mut self`.
            let mutable = match &recv.kind {
                syn::ReceiverKind::Reference(_, _, m) => m.is_some(),
                _ => {
                    skipped.push(Skip::new(&name, None, Reason::ConsumesSelf));
                    continue;
                }
            };

            let mut args = Vec::new();
            let mut ok = true;
            for a in inputs {
                let syn::FnArg::Typed(t) = a else { continue };
                let ty = ty_str(&t.ty);
                // A generic parameter is classified through its bound instead.
                let bound = bounds.get(&ty).map(String::as_str).unwrap_or("");
                let kind = if bound.contains("AsRef<std::path::Path>") {
                    Some(Arg::PathStr)
                } else if bound.contains("Into<String>") {
                    Some(Arg::Str)
                } else {
                    match vocab.classify(&ty) {
                        Some(Js::Bytes) => Some(Arg::Bytes),
                        Some(Js::Str) => Some(Arg::Str),
                        _ if standin_cfg.is_some_and(|s| ty == s.0) => Some(Arg::Standin),
                        _ => None,
                    }
                };
                match kind {
                    Some(k) => args.push((t.pat.clone(), k)),
                    None => {
                        ok = false;
                        skipped.push(Skip::new(&name, Some(&ty), Reason::Arg));
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }

            // What the method returns, settled before anything is built: a
            // return with no JS form skips the method.
            let ret = match &f.sig.output {
                ReturnType::Default => None,
                ReturnType::Type(_, t) => {
                    let ty = ty_str(t);
                    match vocab.classify(&ty) {
                        Some(js)
                            if js_conv(&js, &ty, vocab, &TokenStream::new(), Site::Method)
                                .is_some() =>
                        {
                            Some((js, ty))
                        }
                        _ => {
                            skipped.push(Skip::new(&name, Some(&ty), Reason::Returns));
                            continue;
                        }
                    }
                }
            };
            let unit = ret.is_none();

            let ident = &f.sig.ident;
            let params: Vec<TokenStream> = args
                .iter()
                .map(|(pat, kind)| match kind {
                    Arg::Bytes => quote!(#pat: Uint8Array),
                    Arg::Standin => {
                        let (class, _) = standin.as_ref().expect("a stand-in is configured");
                        quote!(#pat: &#class)
                    }
                    _ => quote!(#pat: String),
                })
                .collect();
            let fwd: Vec<TokenStream> = args
                .iter()
                .map(|(pat, kind)| match kind {
                    Arg::Bytes => quote!(#pat.to_vec()),
                    _ => quote!(#pat),
                })
                .collect();
            // A stand-in is resolved against the receiver: a key into a slot
            // map, say, where another map's key could name a live slot here.
            // Resolved before the call, not inside it: a `DerefMut` receiver
            // (a copy-on-write handle) cannot be borrowed again in its own
            // arguments.
            let lets: Vec<TokenStream> = args
                .iter()
                .filter(|(_, kind)| matches!(kind, Arg::Standin))
                .map(|(pat, _)| {
                    let (_, resolve) = standin.as_ref().expect("a stand-in is configured");
                    quote!(let #pat = #pat.#resolve(&#receiver);)
                })
                .collect();
            if readonly && mutable {
                skipped.push(Skip::new(&name, None, Reason::NeedsMut));
                continue;
            }
            let recv = if mutable {
                quote!(&mut self)
            } else {
                quote!(&self)
            };
            let doc = docs(&f.attrs);
            let on = match receiver_mut {
                Some(r) if mutable => r,
                _ => receiver,
            };
            let call = quote!(#on.#ident(#(#fwd),*));

            // The JS type and the expression producing it: the one table.
            let (ret_ty, value, already_result) = match &ret {
                None => (quote!(()), quote!(#call), false),
                Some((js, ty)) => {
                    let c = js_conv(js, ty, vocab, &call, Site::Method).expect("checked above");
                    (c.ty, c.expr, c.is_result)
                }
            };

            // The signature's return and the body's last expression. A gated
            // body has an `Err` twin, so it always returns `Result`.
            let (arrow, last) = if unit && gate.is_some() {
                (quote!(-> Result<#ret_ty>), quote! { #value; Ok(()) })
            } else if already_result {
                // TryUnit's value is already the Result.
                (quote!(-> Result<#ret_ty>), value)
            } else if gate.is_some() {
                (quote!(-> Result<#ret_ty>), quote!(Ok(#value)))
            } else if unit {
                (quote!(), value)
            } else {
                (quote!(-> #ret_ty), value)
            };
            let body = quote! {
                #(#lets)*
                #last
            };
            // Every path from here emits the method.
            taken.insert(name.clone());
            let (cfg_attr, body) = match gate {
                None => (None, body),
                Some((p, n)) => {
                    let missing = format!(
                        "{} is not available on this target: its upstream feature is off here",
                        lower_camel(&name)
                    );
                    (
                        Some(quote!(#[cfg_attr(#n, allow(unused_variables))])),
                        quote! {
                            #[cfg(#p)]
                            { #body }
                            #[cfg(#n)]
                            { Err(Error::from_reason(#missing)) }
                        },
                    )
                }
            };
            code.extend(quote! {
                #(#doc)*
                #[napi]
                #cfg_attr
                pub fn #ident(#recv, #(#params),*) #arrow {
                    #body
                }
            });
        }
    }

    (code, skipped)
}

/// The cfg a method's body is built under, and its negation, when upstream
/// gates it on a feature this crate does not build everywhere; `None` when
/// it is always there. Its twins in the same impl (one method written once
/// per setting of a feature) count together.
///
/// napi-derive registers every method of a `#[napi] impl` whatever its
/// `#[cfg]`, so a gated method cannot be compiled out. It is kept on every
/// target, with its body gated instead: where the upstream item is missing
/// it throws, and the JS surface stays the same everywhere.
pub fn method_gate(imp: &syn::ItemImpl, f: &syn::ImplItemFn) -> Option<(TokenStream, TokenStream)> {
    let gated = crate::cfg::Pred::Any(
        imp.items
            .iter()
            .filter_map(|i| match i {
                ImplItem::Fn(g) if g.sig.ident == f.sig.ident => Some(crate::cfg::Pred::All(vec![
                    crate::cfg::of(&imp.attrs),
                    crate::cfg::of(&g.attrs),
                ])),
                _ => None,
            })
            .collect(),
    );
    match crate::cfg::gate(&gated) {
        crate::cfg::Gate::Always => None,
        crate::cfg::Gate::When(p, n) => Some((p, n)),
    }
}

/// The keys of an item's `#[napi(..)]`, with their string values:
/// `#[napi(getter, js_name = "type")]` is `{getter: None, js_name: "type"}`.
/// None without a `#[napi]`; a bare one has no keys.
pub(crate) fn napi_args(attrs: &[syn::Attribute]) -> Option<BTreeMap<String, Option<String>>> {
    let attr = attrs.iter().find(|a| a.path().is_ident("napi"))?;
    let mut out = BTreeMap::new();
    if matches!(attr.meta, syn::Meta::List(_)) {
        attr.parse_nested_meta(|meta| {
            let value = if meta.input.peek(syn::Token![=]) {
                match meta.value()?.parse::<syn::Expr>()? {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) => Some(s.value()),
                    _ => None,
                }
            } else {
                None
            };
            let key = &meta.path;
            out.insert(quote!(#key).to_string(), value);
            Ok(())
        })
        .unwrap_or_else(|e| panic!("#[napi(..)] that does not parse: {e}"));
    }
    Some(out)
}

/// The name JS sees a method under: its `js_name`, or napi's camelCase.
fn js_name(f: &syn::ImplItemFn) -> String {
    napi_args(&f.attrs)
        .and_then(|mut a| a.remove("js_name").flatten())
        .unwrap_or_else(|| lower_camel(&f.sig.ident.to_string()))
}

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
                Some((body, js_name(f)))
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
            t.core.to_string().to_case(convert_case::Case::Pascal)
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
                    #[napi(ts_arg_type = "AbortSignal | undefined | null")] signal: Option<Signal>,
                ) -> AsyncTask<#task> {
                    AsyncTask::with_optional_signal(
                        #task { recv: self.clone(), #(#names,)* },
                        signal.map(|s| s.0),
                    )
                }
            }
        });
    }
    out
}

/// Puts `self.live()?` first in every JS-facing method of `class` but `open`,
/// so a disposed instance throws instead of answering from the empty tree it
/// was left with. A method not returning `Result` is made to: napi unwraps it,
/// so the `.d.ts` does not change.
pub fn guard_live(file: &mut syn::File, class: &str, open: &[&str]) {
    for item in &mut file.items {
        let Item::Impl(imp) = item else { continue };
        if imp.trait_.is_some() || !self_ident(imp).is_some_and(|id| id == class) {
            continue;
        }
        for it in &mut imp.items {
            let ImplItem::Fn(f) = it else { continue };
            if !f.attrs.iter().any(|a| a.path().is_ident("napi"))
                || !matches!(f.sig.inputs.first(), Some(syn::FnArg::Receiver(_)))
                || open.contains(&f.sig.ident.to_string().as_str())
            {
                continue;
            }
            let body = &f.block;
            let returns_result =
                matches!(&f.sig.output, ReturnType::Type(_, t) if ty_str(t).starts_with("Result<"));
            f.block = match &f.sig.output {
                _ if returns_result => {
                    let stmts = &body.stmts;
                    syn::parse_quote!({ self.live()?; #(#stmts)* })
                }
                // A closure, so the body's own `return` and `?` keep their meaning.
                _ => {
                    syn::parse_quote!({ self.live()?; #[allow(clippy::redundant_closure_call)] Ok((|| #body)()) })
                }
            };
            if !returns_result {
                let t = match &f.sig.output {
                    ReturnType::Type(_, t) => quote!(#t),
                    ReturnType::Default => quote!(()),
                };
                f.sig.output = syn::parse_quote!(-> Result<#t>);
            }
        }
    }
}

/// What `console.log` prints, for every class: its getters and its cheap
/// methods under its name, and the name as `Symbol.toStringTag`. Read off the
/// finished file, so a class or a member added anywhere -- template or pass --
/// is covered without a list.
///
/// Cheap means no argument and a scalar result: a string, a number, a boolean
/// or a string enum, possibly null. That leaves out what allocates or renders
/// (`children`, `renderPng`, `toString`) and whatever needs a parameter.
pub fn inspect_members(file: &syn::File) -> TokenStream {
    let mut scalars: BTreeSet<String> = [
        "String", "bool", "f64", "f32", "u32", "i32", "u8", "u16", "i64", "u64", "usize",
    ]
    .map(str::to_string)
    .into();
    for item in &file.items {
        if let Item::Enum(e) = item
            && napi_args(&e.attrs).is_some_and(|a| a.contains_key("string_enum"))
        {
            scalars.insert(e.ident.to_string());
        }
    }
    let scalar = |ret: &ReturnType| {
        let ReturnType::Type(_, t) = ret else {
            return false;
        };
        // Result and Option only, not peel_type: a list is not cheap.
        let mut name = ty_str(t);
        while let Some(inner) = ["Result<", "Option<"]
            .iter()
            .find_map(|w| name.strip_prefix(w).and_then(|r| r.strip_suffix('>')))
        {
            // `Result<T, String>`: a string error code, the value is `T`.
            name = inner.strip_suffix(",String").unwrap_or(inner).to_string();
        }
        scalars.contains(&name)
    };
    // `#[napi(object)]` and the enums are plain values, which print fine
    // already.
    let classes: Vec<(&syn::Ident, String, Option<String>)> =
        file.items.iter().filter_map(class_of).collect();
    let mut out = TokenStream::new();
    for (class, js_class, namespace) in classes {
        let (mut getters, mut methods) = (Vec::new(), Vec::new());
        let name = class.to_string();
        // An impl lands on its class in the `.d.ts` only under the same
        // namespace.
        let attr = match &namespace {
            Some(ns) => quote!(#[napi(namespace = #ns)]),
            None => quote!(#[napi]),
        };
        let members = inherent_impls(std::slice::from_ref(file), &name).flat_map(|imp| &imp.items);
        for it in members {
            let ImplItem::Fn(f) = it else { continue };
            let Some(args) = napi_args(&f.attrs) else {
                continue;
            };
            let js = js_name(f);
            if args.contains_key("getter") {
                getters.push(js);
            } else if is_pub(&f.vis)
                && !["setter", "constructor", "factory"]
                    .iter()
                    .any(|k| args.contains_key(*k))
                // `&self` only: printing must not write or consume.
                && matches!(
                    f.sig.inputs.first(),
                    Some(syn::FnArg::Receiver(r))
                        if matches!(r.kind, syn::ReceiverKind::Reference(_, _, None))
                )
                && f.sig.inputs.len() == 1
                && scalar(&f.sig.output)
            {
                methods.push(js);
            }
        }
        out.extend(quote! {
            #attr
            impl #class {
                #[doc = " What `console.log` and `util.inspect` print: the getters, under the class name."]
                #[napi(symbol_for = "nodejs.util.inspect.custom")]
                pub fn inspect_custom(
                    &self,
                    env: Env,
                    this: This,
                    depth: Option<f64>,
                    options: Object,
                    inspect: Function<FnArgs<(Object, Object)>, String>,
                ) -> Result<String> {
                    napi_support::inspect(&env, &this, #js_class, &[#(#getters),*], &[#(#methods),*], depth, options, inspect)
                }
                #[doc = " The class name, for `Object.prototype.toString`."]
                #[napi(getter, well_known_symbol = "toStringTag")]
                pub fn to_string_tag(&self) -> String {
                    #js_class.to_string()
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
                // Private helpers are not API: one returning a borrowed
                // upstream node must not make the node a data candidate.
                if !is_pub(&f.vis) {
                    continue;
                }
                let ReturnType::Type(_, t) = &f.sig.output else {
                    continue;
                };
                // Result<Option<Text>> -> Text, in any nesting order.
                out.insert(peel_type(&ty_str(t)).to_string());
            }
        }
    }
    out
}

/// The second surface: every item of `parts` again, each under its part's JS
/// namespace (`toy.Point`), beside the first surface it is copied from.
/// Returns the code, and what was left out with the reason.
///
/// Rust forbids two items of one name, so each copied item is renamed with
/// its namespace as a prefix -- `Point` becomes `ToyPoint`, a free function
/// `point_to_js` becomes `toy_point_to_js` -- and given `js_name` and
/// `namespace`, so the prefix stays out of JavaScript (napi's `.d.ts` still
/// aliases a class's Rust name to it, as a type). The JS name is the upstream
/// one: a `Config::data_renames` or `Config::enum_renames` entry is undone.
///
/// References between copied items are renamed with them, across parts, so a
/// type of one namespace holding a type of another points at the other's
/// copy. A path (`a::Point`), a field or a method (`.point`) and an enum's
/// variants are never renamed: those name upstream, not the copy.
///
/// napi keeps one constructor per JS class name, namespace or not (see
/// [`class_names`]), so a copied class may not share its JS name with a
/// class of `flat` or of an earlier part: such a copy is left out, and so is
/// everything that names a left-out item.
///
/// napi resolves a reference to its TypeScript name when the referencing
/// item expands, from what has expanded before it, and falls back to the
/// Rust identifier. The copies' struct and enum definitions are therefore
/// put before whatever names them, and every impl and function after all of
/// them.
pub fn namespaced(
    parts: &[(&str, TokenStream)],
    flat: &TokenStream,
    shared: &BTreeSet<String>,
) -> Surfaces {
    // The flat classes, by JS name.
    let flat_classes = class_names(flat);
    let mut report: Vec<String> = Vec::new();
    let parsed: Vec<(&str, syn::File)> = parts
        .iter()
        .map(|(ns, code)| {
            let file = syn::parse2(code.clone())
                .unwrap_or_else(|e| panic!("namespace {ns}: not valid Rust: {e}"));
            (*ns, file)
        })
        .collect();
    // The upstream name behind a renamed first-surface one.
    let upstream = |ident: &str| -> String {
        config()
            .data_renames
            .iter()
            .chain(config().enum_renames)
            .find(|(_, to)| *to == ident)
            .map_or(ident, |(from, _)| from)
            .to_string()
    };
    // An item is keyed by its first-surface ident and its namespace: two
    // namespaces may each define a `Color`. The ident first, so the keys sort
    // as the idents do.
    let key = |id: &str, ns: &str| format!("{id}#{ns}");
    // key -> (copy's ident, JS name, namespace)
    let mut names: BTreeMap<String, (proc_macro2::Ident, String, &str)> = BTreeMap::new();
    // What is left out, with the reason; the rest follows from it below.
    let mut out_why: BTreeMap<String, String> = BTreeMap::new();
    let mut classes: BTreeSet<String> = BTreeSet::new();
    for (ns, file) in &parsed {
        let pascal = ns.to_case(convert_case::Case::Pascal);
        let snake = ns.to_case(convert_case::Case::Snake);
        for item in &file.items {
            // The prefix goes on the JS name, so the Rust name says what JS
            // sees: `FaceInfo` copied from a first-surface `FontFace` is
            // `<Ns>FaceInfo`.
            let (id, new) = match item {
                Item::Struct(s) => (
                    &s.ident,
                    format_ident!("{pascal}{}", upstream(&s.ident.to_string())),
                ),
                Item::Enum(e) => (
                    &e.ident,
                    format_ident!("{pascal}{}", upstream(&e.ident.to_string())),
                ),
                Item::Fn(f) => (&f.sig.ident, format_ident!("{snake}_{}", f.sig.ident)),
                _ => continue,
            };
            let js = upstream(&id.to_string());
            // Taken on the flat surface, or by an earlier namespace -- unless
            // `shared` lets two namespaces have the name: classes only ever
            // built by the class they are called on.
            if class_of(item).is_some()
                && (flat_classes.contains(&js)
                    || (!shared.contains(&js) && !classes.insert(js.clone())))
            {
                out_why.insert(
                    key(&id.to_string(), ns),
                    format!("not namespaced: {ns}.{js}: a class named {js} exists already"),
                );
            }
            let prior = names.insert(key(&id.to_string(), ns), (new, js, ns));
            assert!(prior.is_none(), "{id} is defined twice in namespace {ns}");
        }
    }
    // A copy's ident -> its key.
    let keys: BTreeMap<String, String> = names
        .iter()
        .map(|(k, (new, _, _))| (new.to_string(), k.clone()))
        .collect();

    // (key or empty, whether a definition, the item renamed, the keys of the
    // copies it names)
    let mut items: Vec<(String, bool, TokenStream, BTreeSet<String>)> = Vec::new();
    for (ns, file) in &parsed {
        // What a name means in this part: its own item, or else the one
        // other namespace's item of that name. One that two other namespaces
        // define stays as it is.
        let mut renames: BTreeMap<String, proc_macro2::Ident> = BTreeMap::new();
        let mut elsewhere: BTreeMap<&str, Vec<&proc_macro2::Ident>> = BTreeMap::new();
        for (k, (new, _, n)) in &names {
            let id = k.rsplit_once('#').map_or(k.as_str(), |(id, _)| id);
            if n == ns {
                renames.insert(id.to_string(), new.clone());
            } else {
                elsewhere.entry(id).or_default().push(new);
            }
        }
        for (id, news) in elsewhere {
            if let [new] = news[..] {
                renames.entry(id.to_string()).or_insert_with(|| new.clone());
            }
        }
        for item in &file.items {
            let mut item = item.clone();
            let own = match &mut item {
                Item::Struct(s) => Some((s.ident.to_string(), &mut s.attrs)),
                Item::Enum(e) => Some((e.ident.to_string(), &mut e.attrs)),
                _ => None,
            };
            if let Some((id, attrs)) = own {
                let (_, js, ns) = &names[&key(&id, ns)];
                with_napi_args(attrs, quote!(js_name = #js, namespace = #ns));
            } else if let Item::Impl(imp) = &mut item {
                // An impl joins its class in the `.d.ts` only under the
                // class's namespace.
                let ns = self_ident(imp)
                    .and_then(|id| renames.get(&id.to_string()))
                    .and_then(|new| names.get(&keys[&new.to_string()]))
                    .map_or(*ns, |(_, _, n)| n);
                with_napi_args(&mut imp.attrs, quote!(namespace = #ns));
            }
            let is_def = matches!(item, Item::Struct(_) | Item::Enum(_));
            let id = match item_name(&item) {
                n if n.is_empty() => n,
                n => key(&n, ns),
            };
            let mut named = BTreeSet::new();
            let code = rename_idents(quote!(#item), &renames, &mut named);
            let mut named: BTreeSet<String> = named.iter().map(|n| keys[n].clone()).collect();
            named.remove(&id);
            items.push((id, is_def, code, named));
        }
    }
    // Whatever names a left-out item goes too, to a fixpoint: an impl of it, a
    // type holding it, a function returning it.
    loop {
        let more: Vec<(String, String)> = items
            .iter()
            .filter(|(id, _, _, _)| !id.is_empty() && !out_why.contains_key(id))
            .filter_map(|(id, _, _, named)| {
                let gone = named.iter().find(|n| out_why.contains_key(*n))?;
                let (_, js, ns) = &names[id];
                Some((
                    id.clone(),
                    format!("not namespaced: {ns}.{js}: it names {}", names[gone].1),
                ))
            })
            .collect();
        if more.is_empty() {
            break;
        }
        out_why.extend(more);
    }
    items.retain(|(id, _, _, named)| {
        !out_why.contains_key(id) && !named.iter().any(|n| out_why.contains_key(n))
    });
    let (defs, rest): (Vec<_>, Vec<_>) = items.into_iter().partition(|(_, def, _, _)| *def);

    // Definitions in dependency order, the parts' own order otherwise. A cycle
    // (a type holding a list of itself) is cut where it is first met.
    type Def = (String, bool, TokenStream, BTreeSet<String>);
    fn visit(
        i: usize,
        defs: &[Def],
        index: &BTreeMap<&str, usize>,
        state: &mut [u8],
        out: &mut TokenStream,
    ) {
        if state[i] != 0 {
            return;
        }
        state[i] = 1;
        for dep in &defs[i].3 {
            if let Some(&j) = index.get(dep.as_str()) {
                visit(j, defs, index, state, out);
            }
        }
        state[i] = 2;
        out.extend(defs[i].2.clone());
    }
    let index: BTreeMap<&str, usize> = defs
        .iter()
        .enumerate()
        .map(|(i, (id, _, _, _))| (id.as_str(), i))
        .collect();
    let mut out = TokenStream::new();
    let mut state = vec![0u8; defs.len()];
    for i in 0..defs.len() {
        visit(i, &defs, &index, &mut state, &mut out);
    }
    out.extend(rest.into_iter().map(|(_, _, code, _)| code));
    report.extend(out_why.into_values());
    Surfaces {
        namespaced: out,
        report,
    }
}

/// What [`namespaced`] makes of the parts.
pub struct Surfaces {
    /// The copies, under their namespaces.
    pub namespaced: TokenStream,
    /// One line per item left out (`not namespaced`).
    pub report: Vec<String>,
}

/// A `#[napi]` struct that is a class, not an object: its Rust ident, its JS
/// name and its namespace.
pub(crate) fn class_of(item: &Item) -> Option<(&syn::Ident, String, Option<String>)> {
    let Item::Struct(s) = item else { return None };
    let mut args = napi_args(&s.attrs)?;
    if ["object", "transparent", "array"]
        .iter()
        .any(|k| args.contains_key(*k))
    {
        return None;
    }
    let js = args
        .remove("js_name")
        .flatten()
        .unwrap_or_else(|| s.ident.to_string());
    Some((&s.ident, js, args.remove("namespace").flatten()))
}

/// The JS names of the classes `code` declares.
///
/// napi keeps one constructor per JS class name, whatever its namespace, and
/// builds every instance it returns from the constructor registered under
/// that name: two classes sharing one leave one of them handing out
/// instances of the other. [`namespaced`] reads them off the flat surface to
/// stay clear of.
pub fn class_names(code: &TokenStream) -> BTreeSet<String> {
    let file: syn::File = syn::parse2(code.clone()).expect("generated code that parses");
    file.items
        .iter()
        .filter_map(class_of)
        .map(|(_, js, _)| js)
        .collect()
}

/// The name a struct, enum or function declares, or empty.
fn item_name(item: &Item) -> String {
    match item {
        Item::Struct(s) => s.ident.to_string(),
        Item::Enum(e) => e.ident.to_string(),
        Item::Fn(f) => f.sig.ident.to_string(),
        _ => String::new(),
    }
}

/// Adds `extra` to an item's `#[napi]` or `#[napi(..)]`; nothing without one.
fn with_napi_args(attrs: &mut [syn::Attribute], extra: TokenStream) {
    let Some(attr) = attrs.iter_mut().find(|a| a.path().is_ident("napi")) else {
        return;
    };
    *attr = match &attr.meta {
        syn::Meta::List(l) => {
            let have = &l.tokens;
            syn::parse_quote!(#[napi(#have, #extra)])
        }
        _ => syn::parse_quote!(#[napi(#extra)]),
    };
}

/// `ts` with every identifier in `renames` replaced, except where it names
/// something else: after `::` (an upstream path, a variant), after `.` (a
/// field, a method), or as a variant of an enum being declared. What was
/// renamed is added to `named`.
fn rename_idents(
    ts: TokenStream,
    renames: &BTreeMap<String, proc_macro2::Ident>,
    named: &mut BTreeSet<String>,
) -> TokenStream {
    use proc_macro2::{Delimiter, Group, TokenTree};
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let punct =
        |i: usize, c: char| matches!(toks.get(i), Some(TokenTree::Punct(p)) if p.as_char() == c);
    let mut out = Vec::with_capacity(toks.len());
    // Between `enum` and the enum's body, which is left alone.
    let mut enum_head = false;
    for (i, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Ident(id) => {
                let path = i >= 2 && punct(i - 1, ':') && punct(i - 2, ':');
                let member = i >= 1 && punct(i - 1, '.');
                match renames.get(&id.to_string()) {
                    Some(new) if !path && !member => {
                        named.insert(new.to_string());
                        let mut new = new.clone();
                        new.set_span(id.span());
                        out.push(TokenTree::Ident(new));
                    }
                    _ => out.push(t.clone()),
                }
                if id == "enum" {
                    enum_head = true;
                }
            }
            TokenTree::Group(g) if enum_head && g.delimiter() == Delimiter::Brace => {
                enum_head = false;
                out.push(t.clone());
            }
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), rename_idents(g.stream(), renames, named));
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            _ => out.push(t.clone()),
        }
    }
    out.into_iter().collect()
}
