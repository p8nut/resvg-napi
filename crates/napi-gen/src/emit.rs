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
use quote::{ToTokens, format_ident, quote};
use std::collections::{BTreeMap, BTreeSet};
use syn::{Fields, ImplItem, Item, ReturnType, Type};

use crate::Config;
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
/// the discovery fixpoint), and whether the expression is already a `Result`.
pub struct Conv {
    pub ty: TokenStream,
    pub expr: TokenStream,
    pub reached: Option<String>,
    pub is_result: bool,
}

/// The one Js -> JS table. `access` is the Rust expression to convert; `src_ty`
/// its type, for the `&` it may already carry. None: no form at this site.
pub fn js_conv(
    js: &Js,
    src_ty: &Type,
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
    };
    let reaching = |ty: TokenStream, expr: TokenStream, t: &str| Conv {
        reached: Some(t.to_string()),
        ..plain(ty, expr)
    };
    let is_ref = matches!(Shape::of(src_ty), Shape::Ref { .. });
    // What an `Option` member holds, a plain `&` aside.
    let opt = match Shape::of(unref(src_ty)) {
        Shape::Option(inner) => Some(Shape::of(inner)),
        _ => None,
    };
    // `&x` unless the member already is a reference.
    let by_ref = if is_ref { quote!(#a) } else { quote!(&#a) };
    let saturating = quote!(u32::try_from(#a).unwrap_or(u32::MAX));
    Some(match (js, site) {
        (Js::Bool, _) => plain(quote!(bool), d),
        (Js::I32, _) => plain(quote!(i32), d),
        (Js::I16, _) => plain(quote!(i32), quote!(i32::from(#d))),
        (Js::F32, _) => plain(quote!(f64), quote!(f64::from(#d))),
        (Js::F64, _) => plain(quote!(f64), d),
        // A method's integers saturate; a member's widen, which cannot fail.
        (Js::U8 | Js::U16 | Js::U32, Method) => plain(quote!(u32), saturating),
        (Js::U8 | Js::U16, _) => plain(quote!(u32), quote!(u32::from(#d))),
        (Js::U32, _) => plain(quote!(u32), d),
        (Js::Count, Carried) => return None,
        (Js::Count, _) => plain(quote!(u32), saturating),
        (Js::Str, _) => plain(quote!(String), quote!(#a.to_string())),
        (Js::Path, _) => plain(quote!(String), quote!(#a.to_string_lossy().into_owned())),
        (Js::Scalar, _) => plain(quote!(f64), quote!(f64::from(#a.get()))),
        (Js::IntNewtype(_), Carried) => return None,
        (Js::IntNewtype(_), _) => plain(quote!(u32), quote!(u32::from(#a.0))),
        // A class cannot be an object field or a union's payload: a
        // plain-data one is its `<Name>Object` there, the pair of its
        // `toObject` (`class.rs`).
        (Js::Class(t), Field) if vocab.plain.contains(t) => {
            let o = format_ident!("{t}Object");
            plain(quote!(#o), quote!(#o::from(#by_ref)))
        }
        (Js::Class(t), Carried) if vocab.plain.contains(t) => {
            let o = format_ident!("{t}Object");
            plain(quote!(#o), quote!(#o::from(#a)))
        }
        (Js::Class(_), Field | Carried) => return None,
        (Js::Class(t), Getter | Method) => {
            let c = format_ident!("{t}");
            let v = if is_ref { quote!(*#a) } else { quote!(#a) };
            plain(quote!(#c), quote!(#c::__from(#v)))
        }
        // A copy of the borrowed value, wrapped as the class wraps its own.
        (Js::Owned(t), Getter | Method | Carried) => {
            let c = format_ident!("{t}");
            plain(quote!(#c), quote!(#c::__from((#d).clone())))
        }
        (Js::OptClass(t), Getter | Method) => {
            let c = format_ident!("{t}");
            let f = if matches!(opt, Some(Shape::Ref { .. })) {
                quote!(|x| #c::__from(*x))
            } else {
                quote!(#c::__from)
            };
            plain(quote!(Option<#c>), quote!(#a.map(#f)))
        }
        // Each element as a union's payload is: read through the reference
        // the iteration binds.
        (Js::TupleList(elems), Getter | Field | Method) => {
            let (mut binds, mut tys, mut exprs) = (Vec::new(), Vec::new(), Vec::new());
            for (i, (ty, js)) in elems.iter().enumerate() {
                let x = format_ident!("x{i}");
                let c = js_conv(js, ty, vocab, &quote!(#x), Carried)?;
                binds.push(x);
                tys.push(c.ty);
                exprs.push(c.expr);
            }
            plain(
                quote!(Vec<(#(#tys),*)>),
                quote!(#a.iter().map(|(#(#binds),*)| (#(#exprs),*)).collect()),
            )
        }
        (Js::Iterated(item, via), Getter | Field | Method) => {
            let via = format_ident!("{via}");
            // The item, handed by value.
            let c = js_conv(item, &syn::parse_quote!(_), vocab, &quote!(x), Getter)?;
            let (ty, e) = (c.ty, c.expr);
            plain(quote!(Vec<#ty>), quote!(#a.#via().map(|x| #e).collect()))
        }
        (Js::F32List, Getter | Field) => plain(
            quote!(Vec<f64>),
            quote!(#a.iter().map(|v| f64::from(*v)).collect()),
        ),
        (Js::F32List, Carried) => plain(
            quote!(Vec<f64>),
            quote!(#a.iter().map(|x| f64::from(*x)).collect()),
        ),
        (Js::F64List, Getter | Field | Carried) => plain(quote!(Vec<f64>), quote!(#a.to_vec())),
        (Js::Enum, _) => {
            let e = enum_ident(&key(vocab.resolve(src_ty)).expect("an enum is named by a path"));
            plain(quote!(#e), quote!(#e::from(#d)))
        }
        // A value class owns its upstream value, so a nested one is cloned
        // into its `wrap`.
        (Js::Value(t), Getter | Method) => {
            let c = data_ident(t);
            reaching(quote!(#c), quote!(#c::wrap(#a.clone())), t)
        }
        // Carried alone by a union variant, which is then an output-only
        // object (`payload_enum_code`).
        (Js::Value(t), Carried) => {
            let c = data_ident(t);
            reaching(quote!(#c), quote!(#c::wrap((*#a).clone())), t)
        }
        (Js::ValueList(t), Getter) => {
            let c = data_ident(t);
            let e = quote!(#a.iter().map(|x| #c::wrap(x.clone())).collect());
            reaching(quote!(Vec<#c>), e, t)
        }
        // `iter` for a slice, `into_iter` for an iterator of references.
        (Js::ValueList(t), Method) => {
            let c = data_ident(t);
            let iter = if matches!(Shape::of(src_ty), Shape::Ref { inner, .. } if matches!(Shape::of(inner), Shape::Slice(_)))
            {
                quote!(iter)
            } else {
                quote!(into_iter)
            };
            let e = quote!(#a.#iter().map(|x| #c::wrap(x.clone())).collect());
            reaching(quote!(Vec<#c>), e, t)
        }
        (Js::OptValue(t), Getter | Method) => {
            let c = data_ident(t);
            // A boxed member wraps what the box holds.
            let e = if opt
                .map(|o| match o {
                    Shape::Ref { inner, .. } => Shape::of(inner),
                    o => o,
                })
                .is_some_and(|o| matches!(o, Shape::Box(_)))
            {
                quote!(#a.map(|x| #c::wrap(x.as_ref().clone())))
            } else {
                quote!(#a.map(|x| #c::wrap(x.clone())))
            };
            reaching(quote!(Option<#c>), e, t)
        }
        (Js::PayloadEnumList(t), Getter | Field) => {
            let info = &vocab.payload[t];
            let (ety, conv) = (info.either_ty(t), info.conv_ident(t));
            plain(quote!(Vec<#ety>), quote!(#a.iter().map(#conv).collect()))
        }
        // A union made from a projection, where there is none to step from:
        // one of its own, over the value (a copy of it, when borrowed).
        (Js::PayloadEnum(t), Getter | Field) if vocab.payload[t].projects(vocab) => {
            let info = &vocab.payload[t];
            let (ety, conv) = (info.either_ty(t), info.conv_ident(t));
            let own = if is_ref {
                quote!(#a.clone())
            } else {
                quote!(#a)
            };
            plain(
                quote!(#ety),
                quote!(#conv(napi_support::Projection::owned(#own))),
            )
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
        // it is its handle class, the `Arc` cloned, rather than a copy: the
        // mirror of the toy's `Ink::Stamped(Arc<Stamp>)`. Carried
        // alone only (`payload_blocker`), by an output-only object. A type
        // `Config::projections` lists is that class, owning the `Arc`.
        (Js::Handle(t), Carried) if vocab.with_id.contains(t) => {
            let w = wrapper_ident(t);
            if vocab.config.projections.iter().any(|(_, p)| p == t) {
                plain(
                    quote!(#w),
                    quote!(#w::__proj(napi_support::Projection::of(#a.clone()))),
                )
            } else {
                plain(quote!(#w), quote!(#w::wrap(#a.clone())))
            }
        }
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

/// A value `Vocab::steps_into` hands out as a projection: its JS type, and
/// what makes it from a `napi_support::Projection` of the upstream value.
pub fn stepped(js: &Js, vocab: &Vocab) -> Option<(TokenStream, TokenStream)> {
    if !vocab.steps_into(js) {
        return None;
    }
    Some(match js {
        Js::Owned(t) => {
            let c = format_ident!("{t}");
            (quote!(#c), quote!(#c::__proj))
        }
        Js::Value(t) => {
            let c = data_ident(t);
            (quote!(#c), quote!(#c::__proj))
        }
        Js::PayloadEnum(t) => {
            let info = &vocab.payload[t];
            (info.either_ty(t), info.conv_ident(t).into_token_stream())
        }
        _ => unreachable!("steps_into answers these only"),
    })
}

/// One carried type as an object field: its JS type, and how to convert the
/// binding `access` into it. None when it cannot be a field at all --
/// `payload_blocker` is the authority on *whether*, this is the *how*.
pub fn carried_field(
    vocab: &Vocab,
    ty: &Type,
    access: &TokenStream,
) -> Option<(TokenStream, TokenStream)> {
    let c = js_conv(&vocab.classify(ty)?, ty, vocab, access, Site::Carried)?;
    Some((c.ty, c.expr))
}

/// The Rust name of the alias naming the union of enum `name`: `NodeUnion`.
/// Not the enum's own name, which its input form ([`enum_input`]) may hold
/// in the same namespace: [`namespaced`] tells the two apart by JS name,
/// the enum's for both, and spells a clashing alias out where it is used.
pub fn alias_ident(name: &str) -> proc_macro2::Ident {
    format_ident!("{name}Union")
}

/// `pub type NodeUnion = Either4<..>`, exported as `Node`: napi writes it as
/// `export type Node = NodeGroup | ..`, and every signature naming the alias
/// as `Node`, so the union has a name TypeScript code can use.
pub fn union_alias(name: &str, parts: &[proc_macro2::Ident]) -> TokenStream {
    let a = alias_ident(name);
    let e = match parts.len() {
        2 => format_ident!("Either"),
        n => format_ident!("Either{n}"),
    };
    let doc = format!(" Any variant of `{name}`.");
    quote! {
        #[doc = #doc]
        #[napi(js_name = #name)]
        pub type #a = #e<#(#parts),*>;
    }
}

/// The union for one payload enum: a struct per payload variant, one shared
/// struct for the unit ones, the alias naming their union ([`union_alias`])
/// and the converter that produces them.
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
        let arm = |v: TokenStream| info.either_arm(name, 0, v);
        for (n, _) in info
            .variants
            .iter()
            .filter(|(_, p)| matches!(p, Payload::None))
        {
            let vid = format_ident!("{}", n);
            let tag = lower_camel(n);
            let g = info.gate(n, &vocab.config);
            let value = arm(quote!(#unit { r#type: #tag.to_string() }));
            arms.push(quote!(#g #up::#vid => #value,));
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
        let n_arm = i;
        let arm = |v: TokenStream| info.either_arm(name, n_arm, v);
        let g = info.gate(n, &vocab.config);
        i += 1;

        // What the struct declares, how the arm binds it, and how each field is
        // built. The three shapes differ only here.
        let (decls, pattern, inits) = match payload {
            Payload::None => unreachable!("skipped above"),
            Payload::Value(ty) if vocab.classify(ty).is_none() => {
                report!(
                    vocab.config,
                    "{name}::{n} carries {}, which has no mappable data: emitted without a value",
                    ty_str(ty)
                );
                (Vec::new(), quote!(#up::#vid(_)), Vec::new())
            }
            // A payload that no object field can hold, but a getter can. Only
            // bytes today: `Buffer` is a handle into the JS heap and is not
            // `Clone`, which `#[napi(object)]` requires of every field, so the
            // variant becomes a read-only class. Narrowing still works -- the
            // discriminant is a getter with a literal return type.
            Payload::Value(ty) if matches!(vocab.classify(ty), Some(Js::Bytes)) => {
                let doc = format!(" `{name}::{n}`.");
                let bytes_doc = format!(
                    " The bytes, as {} holds them: it does not decode them, and neither \
does this. Copied out on each read.",
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
                let raw_ty = std_arc(ty);
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
                            napi_support::copied(&self.raw)
                        }
                    }
                });
                let value = arm(quote!(#sid { raw: v.clone() }));
                arms.push(quote!(#g #up::#vid(v) => #value,));
                continue;
            }
            // A class carried alone, or the handle class of an `Arc`-held
            // definition: an output-only object holding it, which napi does
            // not require to be `Clone`.
            Payload::Value(ty)
                if let Some(js @ (Js::Owned(_) | Js::Value(_) | Js::Handle(_))) =
                    vocab.classify(ty) =>
            {
                // Borrowed from the enum, when the union is made from a
                // projection of it (`projects`): the variant found again.
                let step = stepped(&js, vocab);
                let bind = if step.is_some() { quote!(_) } else { quote!(v) };
                let carried = match step {
                    Some((fty, conv)) => {
                        let p = quote!(p.then(|__e| match __e { #up::#vid(__v) => Some(__v), _ => None }));
                        Some((fty, quote!(#conv(#p.expect("the variant matched")))))
                    }
                    None => carried_field(vocab, ty, &quote!(v)),
                };
                let Some((fty, expr)) = carried else {
                    return Err(format!(
                        "{name}::{n} carries {}, which cannot be carried",
                        ty_str(ty)
                    ));
                };
                let doc = format!(" `{name}::{n}`.");
                items.extend(quote! {
                    #[doc = #doc]
                    #[napi(object, object_from_js = false)]
                    pub struct #sid {
                        #[doc = " Discriminant. Narrow on this."]
                        #[napi(ts_type = #ts)]
                        pub r#type: String,
                        pub value: #fty,
                    }
                });
                let value = arm(quote!(#sid {
                    r#type: #tag.to_string(),
                    value: #expr,
                }));
                arms.push(quote!(#g #up::#vid(#bind) => #value,));
                continue;
            }
            Payload::Value(ty) => {
                let access = quote!(v);
                let Some((fty, expr)) = carried_field(vocab, ty, &access) else {
                    return Err(format!(
                        "{name}::{n} carries {}, which cannot be a field",
                        ty_str(ty)
                    ));
                };
                (
                    vec![quote!(pub value: #fty,)],
                    quote!(#up::#vid(v)),
                    vec![quote!(value: #expr,)],
                )
            }
            // A tuple holding bytes: a read-only class, as bytes alone are,
            // whose `value` getter builds the array. It keeps the payload's
            // own values -- an `Arc` around the bytes is a refcount bump --
            // and copies the bytes out only when read.
            Payload::Tuple(tys) if payload.carries_bytes(vocab) => {
                let doc = format!(" `{name}::{n}`.");
                // One field, a tuple: `dispose::slotted` puts a class's one
                // field in a slot.
                let (mut raw_tys, mut binds, mut jstys, mut convs) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for (j, ty) in tys.iter().enumerate() {
                    let b = format_ident!("v{j}");
                    let raw_ty = std_arc(ty);
                    // Each bound by reference, as a union's payload is.
                    let (jty, conv) = if matches!(vocab.classify(ty), Some(Js::Bytes)) {
                        (quote!(Buffer), quote!(napi_support::copied(#b)))
                    } else {
                        let Some((jty, conv)) = carried_field(vocab, ty, &quote!(#b)) else {
                            return Err(format!(
                                "{name}::{n}.{j} carries {}, which cannot be a field",
                                ty_str(ty)
                            ));
                        };
                        (jty, conv)
                    };
                    raw_tys.push(raw_ty);
                    binds.push(b);
                    jstys.push(jty);
                    convs.push(conv);
                }
                items.extend(quote! {
                    #[doc = #doc]
                    #[napi]
                    pub struct #sid {
                        raw: (#(#raw_tys,)*),
                    }
                    #[napi]
                    impl #sid {
                        #[doc = " Discriminant. Narrow on this."]
                        #[napi(getter, js_name = "type", ts_return_type = #ts)]
                        pub fn kind_tag(&self) -> &'static str {
                            #tag
                        }
                        #[doc = " What the variant carries, in order. Bytes are copied out on each read."]
                        #[napi(getter)]
                        pub fn value(&self) -> (#(#jstys),*) {
                            let (#(#binds,)*) = &self.raw;
                            (#(#convs),*)
                        }
                    }
                });
                let value = arm(quote!(#sid {
                    raw: (#(#binds.clone(),)*),
                }));
                arms.push(quote!(#g #up::#vid(#(#binds),*) => #value,));
                continue;
            }
            // Several unnamed fields: `value` is a tuple of them, an array
            // in JS.
            Payload::Tuple(tys) => {
                let (mut binds, mut ftys, mut exprs) = (Vec::new(), Vec::new(), Vec::new());
                for (j, ty) in tys.iter().enumerate() {
                    let id = format_ident!("v{j}");
                    let Some((fty, expr)) = carried_field(vocab, ty, &quote!(#id)) else {
                        return Err(format!(
                            "{name}::{n}.{j} carries {}, which cannot be a field",
                            ty_str(ty)
                        ));
                    };
                    binds.push(id);
                    ftys.push(fty);
                    exprs.push(expr);
                }
                (
                    vec![quote!(pub value: (#(#ftys),*),)],
                    quote!(#up::#vid(#(#binds),*)),
                    vec![quote!(value: (#(#exprs),*),)],
                )
            }
            Payload::Fields(fields) => {
                let mut decls = Vec::new();
                let mut binds = Vec::new();
                let mut inits = Vec::new();
                for (fname, ty) in fields {
                    let id = format_ident!("{}", fname);
                    let access = quote!(#id);
                    let Some((fty, expr)) = carried_field(vocab, ty, &access) else {
                        return Err(format!(
                            "{name}::{n}.{fname} carries {}, which cannot be a field",
                            ty_str(ty)
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
        let value = arm(quote!(#sid {
            r#type: #tag.to_string(),
            #(#inits)*
        }));
        arms.push(quote!(#g #pattern => #value,));
    }

    let parts = info.parts(name);
    if parts.len() > 1 {
        items.extend(union_alias(name, &parts));
    }
    let conv = info.conv_ident(name);
    let ety = info.either_ty(name);
    // A union carrying a value it can borrow is made from a projection of
    // the enum, which that value steps into; the rest read the enum through it.
    items.extend(if info.projects(vocab) {
        quote! {
            // Called only by what returns the enum: a union no member
            // returns (an error's, thrown instead) leaves it unused.
            #[allow(dead_code)]
            fn #conv(p: napi_support::Projection<#up>) -> #ety {
                match p.get() { #(#arms)* }
            }
        }
    } else {
        quote! {
            // Called only by what returns the enum: a union no member
            // returns (an error's, thrown instead) leaves it unused.
            #[allow(dead_code)]
            fn #conv(v: &#up) -> #ety {
                match v { #(#arms)* }
            }
        }
    });
    Ok(items)
}

/// What every generated file starts with, whatever it wraps: napi's imports,
/// `to_f32` (what a JS number becomes as an upstream `f32`, see
/// `class::Prim`), and the global log collection (`napi_support::logs`):
/// `setLogLevel` and `takeLogs`.
pub fn prelude() -> TokenStream {
    quote! {
        use napi::bindgen_prelude::*;
        use napi_derive::napi;

        // ponytail: the one `as` of the runtime. Narrowing a float has no std
        // conversion; `as` rounds to nearest and saturates, which is what a
        // JS number given for an `f32` wants.
        fn to_f32(v: f64) -> f32 {
            v as f32
        }

        #[doc = " Starts collecting what the wrapped crates log."]
        #[doc = ""]
        #[doc = " `level` is `off`, `error`, `warn`, `info`, `debug` or `trace`. Safe to"]
        #[doc = " call repeatedly: the logger is installed once, the level always applies."]
        #[napi(ts_args_type = "level: 'off' | 'error' | 'warn' | 'info' | 'debug' | 'trace'")]
        pub fn set_log_level(level: String) -> Result<()> {
            napi_support::logs::set_level(&level)
        }

        #[doc = " Drains the messages collected since the last call, from every call"]
        #[doc = " and thread: concurrent work interleaves here."]
        #[napi]
        pub fn take_logs() -> Vec<String> {
            napi_support::logs::take()
        }
    }
}

/// `Config::index_bounded`, each suffix with the highest index its table
/// allows, read off the upstream `static` in `files`. A table that is gone
/// fails the build.
pub fn index_bounds(files: &[syn::File], config: &Config) -> Vec<(&'static str, u32)> {
    config
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

/// What [`option_object`] makes of an upstream option struct.
pub struct OptionObject {
    /// The `#[napi(object, object_to_js = false)]` struct and its
    /// `to_upstream`, named as upstream for [`namespaced`] to prefix.
    pub code: TokenStream,
    /// Fields left out, with the reason: they keep the `Default` value.
    pub skipped: Vec<Skip>,
    /// The index-bound suffixes its fields matched, one per field.
    pub bounded: Vec<&'static str>,
}

/// The provider a listed class makes of an option field, with no
/// `Config::providers` entry: `Arc<T>` of a class held behind an `Arc`
/// (`Vocab::shared`), the `Arc` shared; `T` of a class held by value
/// (`Vocab::classes`), copied. Not a plain-data class: JS passes its
/// `<Name>Object` instead. The class by its upstream name, and the
/// expression of `v` giving the value.
fn derived_provider(ty: &Type, vocab: &Vocab) -> Option<(String, &'static str)> {
    let (by_value, of, provide) = match Shape::of(ty) {
        Shape::Arc(inner) => (false, inner, "v.__ref()?.clone()"),
        Shape::Path { .. } => (true, ty, "*v.__ref()?"),
        _ => return None,
    };
    let k = key(of)?;
    let name = bare(&k);
    let provides = if by_value {
        vocab.classes.contains(name) && !vocab.plain.contains(name)
    } else {
        vocab.shared.contains(name)
    };
    provides.then(|| (name.to_string(), provide))
}

/// An upstream option struct with `Default`, as an input-only plain object
/// of the namespaced surface (ADR 0001, "Option structs are plain objects").
///
/// Every field is optional. `to_upstream(&self) -> Result<Upstream, String>` starts
/// from `Default::default()` and overrides only what JS provided, so a field
/// left out keeps upstream's default, whatever it is. A field maps by its
/// type (numbers, strings, enums...), or through `Config::providers` (a class
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
    bounds: &[(&'static str, u32)],
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
    // A field borrowing a list JS passes (`&'a [Family<'a>]`) ties the
    // upstream value to the lists it points into: `'s`, not `'static`.
    let lists = has_input_lists(files, ty);
    let life = if lists { quote!('s) } else { quote!('static) };
    let lifetimes = st.generics.lifetimes().map(|_| life.clone());
    let upstream = upstream_path(ty, modules, root);
    let upstream = quote!(#upstream<#(#lifetimes),*>);
    let (mut list_tys, mut list_inits) = (Vec::new(), Vec::new());
    let name = format_ident!("{ty}");
    let struct_doc = docs(&st.attrs);

    let mut decls = Vec::new();
    let mut assigns = Vec::new();
    let mut skipped = Vec::new();
    let mut bounded = Vec::new();
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
        // Supplied by a class instance: a provider `Config::providers`
        // lists, or one derived from the listed classes. An entry with no
        // `class` only adds its doc to the derived one.
        let listed = vocab.config.providers.iter().find(|p| p.ty == fty);
        let provider = match listed {
            Some(p) if !p.class.is_empty() => Some((p.class.to_string(), p.provide)),
            Some(p) => Some(derived_provider(&f.ty, vocab).unwrap_or_else(|| {
                panic!(
                    "Config::providers: `{}` names no class, and no listed class provides it",
                    p.ty
                )
            })),
            None => derived_provider(&f.ty, vocab),
        };
        if let Some((class, provide)) = provider {
            let doc_extra = listed.map_or("", |p| p.doc);
            let class = format_ident!("{class}");
            let provide: syn::Expr =
                syn::parse_str(provide).expect("a Config::providers expression");
            borrows = true;
            let extra: Vec<TokenStream> = if doc_extra.is_empty() {
                Vec::new()
            } else {
                std::iter::once(String::new())
                    .chain(doc_extra.lines().map(|l| format!(" {l}")))
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
        if let Some(c) = vocab.config.converters.iter().find(|c| c.ty == fty) {
            let js: syn::Type = syn::parse_str(c.js).expect("a Config::converters type");
            let convert = format_ident!("{}", c.convert);
            let (ts, label) = (c.ts, lower_camel(&name));
            decls.push(quote! { #(#doc)* #[napi(ts_type = #ts)] pub #ident: Option<#js>, });
            assigns.push(
                quote! { if let Some(v) = &self.#ident { o.#ident = #convert(v, #label)?; } },
            );
            continue;
        }
        if let Some(e) = input_list(files, &f.ty) {
            let input = format_ident!("{}", e.ident);
            let lt = e.generics.lifetimes().map(|_| quote!('s));
            let up = upstream_path(&e.ident.to_string(), modules, root);
            let k = syn::Index::from(list_tys.len());
            decls.push(quote! { #(#doc)* pub #ident: Option<Vec<#input>>, });
            list_tys.push(quote!(Option<Vec<#up<#(#lt),*>>>));
            list_inits.push(quote! {
                self.#ident
                    .as_ref()
                    .map(|v| v.iter().map(#input::to_upstream).collect::<Result<Vec<_>, String>>())
                    .transpose()?
            });
            assigns.push(quote! { if let Some(v) = &lists.#k { o.#ident = v.as_slice(); } });
            continue;
        }
        match map_field(f, vocab, bounds, &mut bounded) {
            Some(m) => {
                decls.push(m.decl);
                assigns.push(m.assign);
            }
            None => skipped.push(Skip::new(&name, Some(&f.ty), Reason::Unprovided)),
        }
    }
    let (generics, anon) = if borrows {
        (quote!(<'a>), quote!(<'_>))
    } else {
        (TokenStream::new(), TokenStream::new())
    };
    let to_upstream = if lists {
        quote! {
            impl #name #anon {
                #[doc = " The lists the upstream value borrows, converted from JS's."]
                fn lists<'s>(&'s self) -> Result<(#(#list_tys,)*), String> {
                    Ok((#(#list_inits,)*))
                }

                #[doc = " The upstream value: `Default`, with what JS provided on top,"]
                #[doc = " borrowing `lists`."]
                fn to_upstream<'s>(&'s self, lists: &'s (#(#list_tys,)*)) -> Result<#upstream, String> {
                    let mut o = <#upstream>::default();
                    #(#assigns)*
                    Ok(o)
                }
            }
        }
    } else {
        quote! {
            impl #name #anon {
                #[doc = " The upstream value: `Default`, with what JS provided on top."]
                fn to_upstream(&self) -> Result<#upstream, String> {
                    let mut o = <#upstream>::default();
                    #(#assigns)*
                    Ok(o)
                }
            }
        }
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

        #to_upstream
    };
    Some(OptionObject {
        code,
        skipped,
        bounded,
    })
}

/// One option field, or `None` when its type has no JS form here. Every
/// field becomes optional so JS can pass a partial object, and is written
/// back only when JS provided it.
fn map_field(
    f: &syn::Field,
    vocab: &Vocab,
    bounds: &[(&'static str, u32)],
    clamped: &mut Vec<&'static str>,
) -> Option<Field> {
    let ident = f.ident.clone()?;
    let name = ident.to_string();
    let doc = docs(&f.attrs);
    match vocab.classify(&f.ty) {
            Some(Js::F32) => Some(field(
                &ident,
                &doc,
                quote!(f64),
                quote! { if let Some(v) = self.#ident { o.#ident = to_f32(v); } },
            )),
            Some(Js::F64) => Some(field(
                &ident,
                &doc,
                quote!(f64),
                quote! { if let Some(v) = self.#ident { o.#ident = v; } },
            )),
            Some(Js::U8) => {
                // A number that is not a u8 throws, as a u8 argument does. A
                // `Config::index_bounded` field indexes an upstream table
                // without a bounds check, so it takes any u32 and clamps it
                // to that table's last index rather than panicking upstream;
                // a negative or fractional one still throws.
                //
                // The suffix is how the field is recognised, and a rename
                // upstream would silently drop the clamp. The caller collects
                // what this matched and fails the build when a suffix matches
                // nothing.
                let label = lower_camel(&name);
                let set = match bounds.iter().find(|(suffix, _)| name.ends_with(suffix)) {
                    Some((suffix, max)) => {
                        clamped.push(*suffix);
                        quote! {
                            u8::try_from(napi_support::int::<u32>(#label, v)?.min(#max)).unwrap_or(u8::MAX)
                        }
                    }
                    None => quote!(napi_support::int::<u8>(#label, v)?),
                };
                Some(field(
                    &ident,
                    &doc,
                    quote!(f64),
                    quote! { if let Some(v) = self.#ident { o.#ident = #set; } },
                ))
            }
            Some(Js::U32) => {
                let label = lower_camel(&name);
                Some(field(
                    &ident,
                    &doc,
                    quote!(f64),
                    quote! { if let Some(v) = self.#ident { o.#ident = napi_support::int::<u32>(#label, v)?; } },
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
                let e = enum_ident(&key(vocab.resolve(&f.ty)).expect("an enum is named by a path"));
                Some(field(
                    &ident,
                    &doc,
                    quote!(#e),
                    quote! { if let Some(v) = self.#ident { o.#ident = v.into(); } },
                ))
            }
            // `Weight(pub u16)`: a number, refused when it does not fit.
            Some(Js::IntNewtype(_)) => {
                let label = lower_camel(&name);
                Some(field(
                    &ident,
                    &doc,
                    quote!(f64),
                    quote! { if let Some(v) = self.#ident { o.#ident.0 = napi_support::int(#label, v)?; } },
                ))
            }
            // A class instance is a `Config::providers` entry, checked before
            // this; nested objects belong on methods, not in a flat config
            // object.
            Some(Js::Bytes)
            | Some(Js::Path)
            | Some(Js::Count)
            | Some(Js::TryUnit)
            | Some(Js::Class(_))
            | Some(Js::OptClass(_))
            | Some(Js::TupleList(_))
            // Unreached by any upstream option field today.
            | Some(Js::Scalar)
            // Read-only: a union describes what a document holds, never what a
            // render option sets.
            | Some(Js::U16)
            | Some(Js::I32)
            | Some(Js::I16)
            | Some(Js::PayloadEnum(_))
            | Some(Js::PayloadEnumList(_))
            | Some(Js::Iterated(..))
            | Some(Js::Tag4)
            | Some(Js::F32List)
            | Some(Js::F64List)
            | Some(Js::Handle(_))
            | Some(Js::HandleList(_))
            | Some(Js::Object(_))
            | Some(Js::ObjectList(_))
            | Some(Js::Value(_))
            | Some(Js::Owned(_))
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
        if let Err(why) = payload_shape(e, &|t| t.clone()) {
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
        // type names, and what `classify` looks up.
        names.insert(e.ident.to_string());
        code.extend(quote! {
            #(#doc)*
            #[napi(string_enum = "camelCase")]
            #[derive(Copy, Clone)]
            // Upstream's variant names, whatever their case.
            #[allow(non_camel_case_types)]
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
/// expression that produces it from a binding named `v`, or, `stepped`, from
/// `__p`, the read-only class's projection of the value.
pub struct Member {
    pub id: proc_macro2::Ident,
    pub jsty: TokenStream,
    pub value: TokenStream,
    pub stepped: bool,
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
/// Whether `ts` names `id` anywhere in it: `Result<(String, Buffer), String>`
/// names `Buffer`.
fn names(ts: TokenStream, id: &str) -> bool {
    ts.into_iter().any(|t| match t {
        proc_macro2::TokenTree::Ident(i) => i == id,
        proc_macro2::TokenTree::Group(g) => names(g.stream(), id),
        _ => false,
    })
}

pub fn napi_name_clash(jsty: &TokenStream) -> Option<(String, String)> {
    const CLASHES: [&str; 5] = ["Path", "BigInt", "Symbol", "Null", "JsFunction"];
    let t: Type = syn::parse2(jsty.clone()).ok()?;
    // `Option<Path>` and `Path`, but not `PathSegment`, `TracePath` or `Vec<Path>`
    // -- napi only rewrites the type itself, so only those two shapes need it.
    let (inner, optional) = match Shape::of(&t) {
        Shape::Option(i) => (key(i)?, true),
        s => (s.key()?, false),
    };
    if !CLASHES.contains(&inner.as_str()) {
        return None;
    }
    // A field keeps its own optionality: napi derives `?` from the Rust type
    // independently of `ts_type`, so spelling the union here would give
    // `underline?: Path | null`. A getter's return type does not, so it does.
    Some((
        inner.clone(),
        if optional {
            format!("{inner} | null")
        } else {
            inner
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

    // `step` borrows the member from `__v`, a reference to the value, when
    // it can be borrowed (a field, an accessor returning a reference).
    let mut push = |name: &str, ty: &Type, access: TokenStream, step: Option<TokenStream>| {
        // `Config::not_data`: conversions such as `Point::to_xy`.
        if vocab.config.not_data.contains(&name) {
            return;
        }
        let js = vocab.classify(ty);
        // The tree itself (`Config::node_types`): a node is what a handle
        // exists for, and following one into the data registry drags every
        // definition type behind it. A type rule rather than the member
        // names, because the next member to return a `Layer` should need no
        // edit here. A node type a listed class copies (`Js::Owned`) is that class, and
        // reaches nothing behind it.
        if key(peel(ty)).is_some_and(|k| vocab.config.node_types.contains(&bare(&k)))
            && !matches!(js, Some(Js::Owned(_)))
        {
            return;
        }
        let id = format_ident!("{}", name);
        // `Frame` was a read-only class only because `size` did not map;
        // mapping it promoted Frame to an object holding `Content`, whose
        // byte variants are classes. Say so instead of relying on luck.
        if let Some(Js::PayloadEnum(t) | Js::PayloadEnumList(t)) = &js
            && !nested_values
            && vocab.payload[t].holds_class(vocab)
        {
            skipped.push(Skip::new(name, Some(ty), Reason::UnionWithClasses));
            return;
        }
        // A read-only class holds a projection of its value: a member it can
        // borrow and hand out as a projection is one step further, not a copy.
        if nested_values
            && let (Some(js), Some(step)) = (&js, &step)
            && let Some((jsty, conv)) = stepped(js, vocab)
        {
            if let Js::Value(t) = js {
                reached.insert(t.clone());
            }
            out.push(Member {
                id,
                jsty,
                value: quote!(#conv(__p.then(|__v| Some(#step)).expect("a borrow resolves"))),
                stepped: true,
            });
            return;
        }
        let site = if nested_values {
            Site::Getter
        } else {
            Site::Field
        };
        if let Some(c) = js
            .as_ref()
            .and_then(|j| js_conv(j, ty, vocab, &access, site))
        {
            reached.extend(c.reached);
            out.push(Member {
                id,
                jsty: c.ty,
                value: c.expr,
                stepped: false,
            });
            return;
        }
        // `Option<T>` where T maps stays optional on the JS side.
        let raw = match Shape::of(unref(ty)) {
            Shape::Option(inner) => Some(inner),
            _ => None,
        };
        let is_ref = raw.is_some_and(|i| matches!(Shape::of(i), Shape::Ref { .. }));
        match raw
            .map(unref)
            .and_then(|i| vocab.classify(i).map(|j| (i, j)))
        {
            Some((_, Js::F32)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<f64>),
                    value: quote!(#access.map(f64::from)),
                    stepped: false,
                });
            }
            Some((_, Js::F64)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<f64>),
                    value: access,
                    stepped: false,
                });
            }
            Some((_, Js::Scalar)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<f64>),
                    value: quote!(#access.map(|x| f64::from(x.get()))),
                    stepped: false,
                });
            }
            Some((_, Js::F32List)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<Vec<f64>>),
                    value: quote!(
                        #access.map(|v| v.iter().map(|x| f64::from(*x)).collect())
                    ),
                    stepped: false,
                });
            }
            Some((_, Js::F64List)) => {
                out.push(Member {
                    id,
                    jsty: quote!(Option<Vec<f64>>),
                    value: quote!(#access.map(|v| v.to_vec())),
                    stepped: false,
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
                    stepped: false,
                });
            }
            _ => {
                skipped.push(Skip::new(name, Some(ty), Reason::Member));
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
    for imp in inherent_impls(files, bare(ty)) {
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
            let step = matches!(Shape::of(o), Shape::Ref { .. }).then(|| quote!(__v.#id()));
            push(&id.to_string(), o, quote!(v.#id()), step);
        }
    }
    if let Some(Fields::Named(named)) = struct_fields_opt(files, bare(ty)) {
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
            let inner = match Shape::of(&f.ty) {
                Shape::Option(inner) => inner,
                _ => &f.ty,
            };
            let copy = matches!(
                vocab.classify(inner),
                Some(
                    Js::F32
                        | Js::F64
                        | Js::U8
                        | Js::U16
                        | Js::U32
                        | Js::I32
                        | Js::I16
                        | Js::Bool
                        | Js::Enum
                        | Js::Scalar
                        | Js::IntNewtype(_)
                        | Js::Class(_)
                )
            );
            let access = if copy {
                quote!(v.#id)
            } else {
                quote!(v.#id.clone())
            };
            push(&id.to_string(), &f.ty, access, Some(quote!(&__v.#id)));
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
/// to be reconstructible. A record whose id, source and style members have
/// no JS form is the case that matters: what does map is still readable.
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
        let v = (!m.stepped).then(|| quote!(let v = __p.get();));
        quote! {
            #attr
            pub fn #id(&self) -> #t {
                let __p = &self.inner;
                #v
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
    // A projection: of a value of its own (`wrap`), or of one borrowed from
    // what it was reached from, which it keeps alive (`__proj`).
    let code = quote! {
        #[doc = #doc]
        #[napi]
        pub struct #name {
            inner: napi_support::Projection<#up>,
        }

        impl #name {
            // Called only where a member hands the class out: the
            // allow is on it alone, not on the generated crate.
            #[allow(dead_code)]
            fn wrap(inner: #up) -> Self {
                Self::__proj(napi_support::Projection::owned(inner))
            }
            fn __proj(inner: napi_support::Projection<#up>) -> Self {
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
                readonly: !mutable,
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
            let t = s.ty.as_ref().filter(|_| s.reason == Reason::Returns)?;
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
            // Called only where a member hands the class out: the
            // allow is on it alone, not on the generated crate.
            #[allow(dead_code)]
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
    /// An `Arc` receiver cannot hand out `&mut`, so drop mutating methods.
    pub readonly: bool,
}

/// Wraps every `pub fn` of `impl <ty>` whose whole signature is transposable.
/// `receiver` is the expression the call hangs off, so the same pass serves
/// a handle's `self.inner` or a borrow such as `self.inner.root()`.
pub fn map_methods(
    m: &MethodPass,
    vocab: &Vocab,
    // Method names already emitted on the target class: two passes can land on
    // the same `impl` block (`Scene::layers` and `Layer::layers` both do).
    taken: &mut BTreeSet<String>,
) -> (TokenStream, Vec<Skip>) {
    // Destructured by name so the body below is unchanged from when these were
    // eight parameters.
    let MethodPass {
        files,
        ty,
        receiver,
        receiver_mut,
        readonly,
    } = *m;
    let mut code = TokenStream::new();
    let mut skipped = Vec::new();
    let mut twins = BTreeSet::new();

    for imp in inherent_impls(files, ty) {
        for it in &imp.items {
            let ImplItem::Fn(f) = it else { continue };
            if !is_pub(&f.vis) {
                continue;
            }
            let name = f.sig.ident.to_string();
            // Already emitted on this class by an earlier pass, or a cfg twin
            // of one seen in this pass. A name is only reserved once its
            // method is emitted (below): reserving it on sight let a method
            // skipped here hide a mappable one of the same name on a later
            // pass or Deref target, with nothing in the report.
            if taken.contains(&name) || !twins.insert(name.clone()) {
                continue;
            }
            let gate = method_gate(imp, f, &vocab.config);

            // generic param -> what its bound takes, so `P: AsRef<Path>` can
            // be classified.
            let bounds = bound_kinds(&f.sig.generics);

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
                let ty = &*t.ty;
                // A generic parameter is classified through its bound instead.
                let kind = match key(ty).and_then(|k| bounds.get(&k)) {
                    Some(Some(Bound::Path)) => Some(Arg::PathStr),
                    Some(Some(Bound::Str)) => Some(Arg::Str),
                    Some(None) => None,
                    None => match vocab.classify(ty) {
                        Some(Js::Bytes) => Some(Arg::Bytes),
                        Some(Js::Str) => Some(Arg::Str),
                        _ => None,
                    },
                };
                match kind {
                    Some(k) => args.push((t.pat.clone(), k)),
                    None => {
                        ok = false;
                        skipped.push(Skip::new(&name, Some(ty), Reason::Arg));
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
                    let ty = &**t;
                    match vocab.classify(ty) {
                        Some(js)
                            if js_conv(&js, ty, vocab, &TokenStream::new(), Site::Method)
                                .is_some() =>
                        {
                            Some((js, ty))
                        }
                        _ => {
                            skipped.push(Skip::new(&name, Some(ty), Reason::Returns));
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
                // A read-only class borrowed from a read-only `Arc`: a step
                // into it, as `Classes::projected` takes one, not a copy.
                Some((Js::Value(t), ty)) | Some((Js::ValueList(t), ty))
                    if readonly
                        && !mutable
                        && fwd.is_empty()
                        && matches!(Shape::of(ty), Shape::Ref { .. }) =>
                {
                    let c = data_ident(t);
                    let slice = matches!(Shape::of(ty), Shape::Ref { inner, .. } if matches!(Shape::of(inner), Shape::Slice(_)));
                    let b = quote!(napi_support::Projection::of(#receiver.clone()));
                    let value = if slice {
                        quote!({
                            let __b = #b;
                            (0..__b.get().#ident().len())
                                .filter_map(|__i| __b.then(move |__s| __s.#ident().get(__i)))
                                .map(#c::__proj)
                                .collect()
                        })
                    } else {
                        quote!(#c::__proj(#b.then(|__s| Some(__s.#ident())).expect("a borrow resolves")))
                    };
                    let ty = if slice { quote!(Vec<#c>) } else { quote!(#c) };
                    (ty, value, false)
                }
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
            let body = last;
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
pub fn method_gate(
    imp: &syn::ItemImpl,
    f: &syn::ImplItemFn,
    config: &Config,
) -> Option<(TokenStream, TokenStream)> {
    let gated = crate::cfg::Pred::Any(
        imp.items
            .iter()
            .filter_map(|i| match i {
                ImplItem::Fn(g) if g.sig.ident == f.sig.ident => Some(crate::cfg::Pred::All(vec![
                    crate::cfg::of(config, &imp.attrs),
                    crate::cfg::of(config, &g.attrs),
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
        // Result and Option only, not `peel`: a list is not cheap.
        let mut t: &Type = t;
        loop {
            t = match Shape::of(t) {
                // `Result<T, String>`: a string error code, the value is `T`.
                Shape::Result { ok, err }
                    if err.is_none_or(|e| key(e).as_deref() == Some("String")) =>
                {
                    ok
                }
                Shape::Option(inner) => inner,
                _ => break,
            };
        }
        key(t).is_some_and(|k| scalars.contains(&k))
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
                // A getter handing out bytes copies them on each read: a
                // whole font, for a face's source. Printing leaves it out.
                if !matches!(&f.sig.output, ReturnType::Type(_, t) if names(t.to_token_stream(), "Buffer"))
                {
                    getters.push(js);
                }
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
                // Not gated on an upstream feature (the `#[cfg_attr]` a
                // gated body carries, see `method_gate`): where the
                // feature is off it throws, and printing must not.
                && !f.attrs.iter().any(|a| a.path().is_ident("cfg_attr"))
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

/// What a constant of type `ty` is in JS, through type aliases: its Rust
/// type for napi and its TypeScript type. A number type that fits a JS
/// number, a `bool` or a string; `None` for any other (ADR 0002, Q21).
pub fn const_type(ty: &Type, vocab: &Vocab) -> Option<(TokenStream, &'static str)> {
    let ty = vocab.resolve(ty);
    let shape = Shape::of(ty);
    match shape.key().as_deref() {
        Some("u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32" | "f64") => {
            Some((ty.to_token_stream(), "number"))
        }
        Some("bool") => Some((ty.to_token_stream(), "boolean")),
        Some("String") => Some((ty.to_token_stream(), "string")),
        // `&str` and `&'static str` alike.
        _ => match shape {
            Shape::Ref { inner, .. } if key(inner).as_deref() == Some("str") => {
                Some((quote!(&str), "string"))
            }
            _ => None,
        },
    }
}

/// A constant `path` of type `ty` as a `#[napi] const`: its JS type and
/// its value, a constant expression. What [`const_type`] maps is read as it
/// is. A `usize` is a `u32`, checked when the binding builds: a constant that
/// does not fit fails the build rather than being cut. A newtype over `f32`
/// is its `get()`, when that is a `const fn`.
fn const_value(ty: &Type, path: &TokenStream, vocab: &Vocab) -> Option<(TokenStream, TokenStream)> {
    if let Some((js_ty, _)) = const_type(ty, vocab) {
        return Some((js_ty, path.clone()));
    }
    let k = key(vocab.resolve(ty))?;
    if k == "usize" {
        let why = format!("{} does not fit a u32", path.to_string().replace(' ', ""));
        return Some((
            quote!(u32),
            quote!({
                assert!(#path <= u32::MAX as usize, #why);
                #path as u32
            }),
        ));
    }
    vocab
        .const_scalars
        .contains(&k)
        .then(|| (quote!(f32), quote!(#path.get())))
}

/// The public constants a source exports ([`ModTree::consts`]), each as a
/// `#[napi] const` of the namespace of the module that exports it, under its
/// Rust name: `toy::LIMIT` is `toy.LIMIT`, `toy::alt::LIMIT` is `alt.LIMIT`.
/// `root` is the source's path (`toy`). A constant of a number type that fits
/// a JS number, a `bool` or a string maps, through type aliases; so does a
/// `usize`, as a `u32` the build checks it fits, and a newtype over `f32`
/// whose `get` is a `const fn`, as its `f32`. Any other is left out, and so
/// is one exported by a module with no namespace. Returns
/// the code by the root it goes under (`toy::alt`, its namespace by
/// [`Config::namespace_of`](crate::Config::namespace_of)), and a report line
/// per constant.
///
/// [`ModTree::consts`]: crate::sources::ModTree::consts
pub fn constants(
    root: &str,
    consts: &[(String, String, syn::ItemConst)],
    vocab: &Vocab,
) -> (Vec<(String, TokenStream)>, Vec<String>) {
    let (mut code, mut report) = (Vec::new(), Vec::new());
    for (module, name, c) in consts {
        let at = if module.is_empty() {
            root.to_string()
        } else {
            format!("{root}::{module}")
        };
        let path: TokenStream = format!("{at}::{name}").parse().expect("a path");
        let Some((js_ty, value)) = const_value(&c.ty, &path, vocab) else {
            report.push(format!(
                "{at}::{name} constant not exposed: {} (no JS value)",
                ty_str(&c.ty)
            ));
            continue;
        };
        let Some(ns) = vocab.config.namespace_of(&at) else {
            report.push(format!(
                "{at}::{name} constant not exposed: no namespace for {at}"
            ));
            continue;
        };
        let id = &c.ident;
        let docs = docs(&c.attrs);
        let gate = crate::cfg::attr(&vocab.config, &c.attrs);
        report.push(format!("{at}::{name} is a constant: {ns}.{name}"));
        code.push((
            at,
            quote! {
                #(#docs)*
                #gate
                #[napi(namespace = #ns)]
                pub const #id: #js_ty = #value;
            },
        ));
    }
    (code, report)
}

/// The surface: every item of `parts`, each under its part's JS namespace
/// (`toy.Point`). Returns the code, and what was left out with the reason.
///
/// Rust forbids two items of one name, so each item is renamed with its
/// namespace as a prefix -- `Point` becomes `ToyPoint`, a free function
/// `point_to_js` becomes `toy_point_to_js` -- and given `js_name` and
/// `namespace`, so the prefix stays out of JavaScript (napi's `.d.ts` still
/// aliases a class's Rust name to it, as a type). The JS name is the upstream
/// one.
///
/// References between items are renamed with them, across parts, so a type
/// of one namespace holding a type of another points at the other's item. A
/// path (`a::Point`), a field or a method (`.point`) and an enum's variants
/// are never renamed: those name upstream.
///
/// napi keeps one constructor per JS class name, namespace or not: it builds
/// every instance it returns from the constructor registered under that
/// name. So a class may not share its JS name with a class of an earlier
/// part, unless `shared` lets it (a class only ever built by the class it is
/// called on): such a class is left out, and so is everything that names a
/// left-out item.
///
/// napi resolves a reference to its TypeScript name when the referencing
/// item expands, from what has expanded before it, and falls back to the
/// Rust identifier. The copies' struct and enum definitions are therefore
/// put before whatever names them, and every impl and function after all of
/// them.
///
/// A constant ([`constants`]) keeps its Rust name: napi exports a `#[napi]
/// const` under its Rust identifier whatever its `js_name` says (napi-derive
/// 3.7), so a prefix would reach JavaScript. Each part's constants go in a
/// module of their own instead (`toy_consts`), where no other part's can
/// clash with them.
pub fn namespaced(parts: &[(&str, TokenStream)], shared: &BTreeSet<String>) -> Surfaces {
    let mut report: Vec<String> = Vec::new();
    let parsed: Vec<(&str, syn::File)> = parts
        .iter()
        .map(|(ns, code)| {
            let file = syn::parse2(code.clone())
                .unwrap_or_else(|e| panic!("namespace {ns}: not valid Rust: {e}"));
            (*ns, file)
        })
        .collect();
    // An item is keyed by its ident and its namespace: two
    // namespaces may each define a `Color`. The ident first, so the keys sort
    // as the idents do.
    let key = |id: &str, ns: &str| format!("{id}#{ns}");
    // key -> (copy's ident, JS name, namespace)
    let mut names: BTreeMap<String, (proc_macro2::Ident, String, &str)> = BTreeMap::new();
    // What is left out, with the reason; the rest follows from it below.
    let mut out_why: BTreeMap<String, String> = BTreeMap::new();
    let mut classes: BTreeSet<String> = BTreeSet::new();
    // Per namespace, each union alias whose JS name a struct or enum of the
    // namespace has (an input form, `enum_input`): a TypeScript type may not
    // share a name with an interface, a class or another type. Its uses get
    // the union spelled out instead, and the alias is left out.
    let mut inlined: BTreeMap<&str, BTreeMap<String, TokenStream>> = BTreeMap::new();
    for (ns, file) in &parsed {
        let pascal = ns.to_case(convert_case::Case::Pascal);
        let snake = ns.to_case(convert_case::Case::Snake);
        let types: BTreeSet<String> = file
            .items
            .iter()
            .filter(|i| matches!(i, Item::Struct(_) | Item::Enum(_)))
            .map(item_name)
            .collect();
        for item in &file.items {
            let (id, new) = match item {
                Item::Struct(s) => (&s.ident, format_ident!("{pascal}{}", s.ident)),
                Item::Enum(e) => (&e.ident, format_ident!("{pascal}{}", e.ident)),
                Item::Fn(f) => (&f.sig.ident, format_ident!("{snake}_{}", f.sig.ident)),
                Item::Type(t) => {
                    let js = alias_js_name(t);
                    if types.contains(&js) {
                        report.push(format!(
                            "not aliased: {ns}.{js}: a type named {js} exists already, \
                             so its union is spelled out"
                        ));
                        let ty = &t.ty;
                        inlined
                            .entry(ns)
                            .or_default()
                            .insert(t.ident.to_string(), quote!(#ty));
                        continue;
                    }
                    let new = format_ident!("{pascal}{}", t.ident);
                    let prior = names.insert(key(&t.ident.to_string(), ns), (new, js, ns));
                    assert!(
                        prior.is_none(),
                        "{} is defined twice in namespace {ns}",
                        t.ident
                    );
                    continue;
                }
                _ => continue,
            };
            let js = id.to_string();
            // Taken by an earlier namespace -- unless `shared` lets two
            // namespaces have the name: classes only ever built by the class
            // they are called on.
            if class_of(item).is_some() && !shared.contains(&js) && !classes.insert(js.clone()) {
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
    let mut consts = TokenStream::new();
    for (ns, file) in &parsed {
        let own: Vec<&Item> = file
            .items
            .iter()
            .filter(|i| matches!(i, Item::Const(_)))
            .collect();
        if !own.is_empty() {
            let module = format_ident!("{}_consts", ns.to_case(convert_case::Case::Snake));
            let doc = format!(" The constants of `{ns}`, apart: they keep their Rust names.");
            // napi's registration is what reads them, and a test build has
            // none: the allow is on this module alone, not on the crate.
            consts.extend(quote! {
                #[doc = #doc]
                #[allow(dead_code)]
                mod #module {
                    use super::*;
                    #(#own)*
                }
            });
        }
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
        let spelled = inlined.get(ns);
        for item in file.items.iter().filter(|i| !matches!(i, Item::Const(_))) {
            if let (Item::Type(t), Some(s)) = (item, spelled)
                && s.contains_key(&t.ident.to_string())
            {
                continue;
            }
            let mut item = match spelled {
                Some(s) => syn::parse2(spell_out(quote!(#item), s)).expect("an item"),
                None => item.clone(),
            };
            let own = match &mut item {
                Item::Struct(s) => Some((s.ident.to_string(), &mut s.attrs)),
                Item::Enum(e) => Some((e.ident.to_string(), &mut e.attrs)),
                _ => None,
            };
            if let Some((id, attrs)) = own {
                let (_, js, ns) = &names[&key(&id, ns)];
                with_napi_args(attrs, quote!(js_name = #js, namespace = #ns));
            } else if let Item::Type(t) = &mut item {
                // Its `js_name` is set already: the enum's.
                with_napi_args(&mut t.attrs, quote!(namespace = #ns));
            } else if let Item::Impl(imp) = &mut item {
                // An impl joins its class in the `.d.ts` only under the
                // class's namespace.
                let ns = self_ident(imp)
                    .and_then(|id| renames.get(&id.to_string()))
                    .and_then(|new| names.get(&keys[&new.to_string()]))
                    .map_or(*ns, |(_, _, n)| n);
                with_napi_args(&mut imp.attrs, quote!(namespace = #ns));
            }
            let is_def = matches!(item, Item::Struct(_) | Item::Enum(_) | Item::Type(_));
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
    out.extend(consts);
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

/// The name a struct, enum, function or type alias declares, or empty.
fn item_name(item: &Item) -> String {
    match item {
        Item::Struct(s) => s.ident.to_string(),
        Item::Enum(e) => e.ident.to_string(),
        Item::Fn(f) => f.sig.ident.to_string(),
        Item::Type(t) => t.ident.to_string(),
        _ => String::new(),
    }
}

/// The JS name of a type alias ([`union_alias`]): its `js_name`, or its ident.
fn alias_js_name(t: &syn::ItemType) -> String {
    napi_args(&t.attrs)
        .and_then(|mut a| a.remove("js_name").flatten())
        .unwrap_or_else(|| t.ident.to_string())
}

/// `ts` with every identifier in `aliases` replaced by the type it aliases,
/// except where it names something else (after `::` or `.`).
fn spell_out(ts: TokenStream, aliases: &BTreeMap<String, TokenStream>) -> TokenStream {
    use proc_macro2::{Group, TokenTree};
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let punct =
        |i: usize, c: char| matches!(toks.get(i), Some(TokenTree::Punct(p)) if p.as_char() == c);
    let mut out = TokenStream::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            TokenTree::Ident(id)
                if !(i >= 2 && punct(i - 1, ':') && punct(i - 2, ':'))
                    && !(i >= 1 && punct(i - 1, '.')) =>
            {
                match aliases.get(&id.to_string()) {
                    Some(ty) => out.extend(ty.clone()),
                    None => out.extend([t.clone()]),
                }
            }
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), spell_out(g.stream(), aliases));
                ng.set_span(g.span());
                out.extend([TokenTree::Group(ng)]);
            }
            _ => out.extend([t.clone()]),
        }
    }
    out
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
/// field, a method), after `fn` inside an item (a method of an impl, which
/// may share a free function's name), or as a variant of an enum being
/// declared. What was renamed is added to `named`.
fn rename_idents(
    ts: TokenStream,
    renames: &BTreeMap<String, proc_macro2::Ident>,
    named: &mut BTreeSet<String>,
) -> TokenStream {
    rename_in(ts, renames, named, false)
}

/// [`rename_idents`], `nested` inside one of the item's groups: a function
/// defined there is a method, never the free function of the same name.
fn rename_in(
    ts: TokenStream,
    renames: &BTreeMap<String, proc_macro2::Ident>,
    named: &mut BTreeSet<String>,
    nested: bool,
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
                let method =
                    nested && i >= 1 && matches!(&toks[i - 1], TokenTree::Ident(f) if f == "fn");
                match renames.get(&id.to_string()) {
                    Some(new) if !path && !member && !method => {
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
                let mut ng = Group::new(g.delimiter(), rename_in(g.stream(), renames, named, true));
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            _ => out.push(t.clone()),
        }
    }
    out.into_iter().collect()
}

/// What one variant of an input enum takes from JS ([`is_enum_input`]).
enum InputField {
    /// `&'a str`, borrowed from `value` for the call.
    BorrowedStr,
    /// `String`, `PathBuf`: `value`, converted with `Into`.
    Str,
    /// `Vec<u8>`: a copy of `bytes`.
    Bytes,
    /// `Arc<Vec<u8>>`, `Arc<dyn AsRef<[u8]> + ..>`: a copy of `bytes`, shared.
    SharedBytes,
}

fn input_field(t: &syn::Type) -> Option<InputField> {
    let u8s = |e: &Type| key(e).as_deref() == Some("u8");
    match Shape::of(t) {
        Shape::Ref {
            mutable: false,
            inner,
            ..
        } if key(inner).as_deref() == Some("str") => Some(InputField::BorrowedStr),
        s if matches!(
            s.key().as_deref(),
            Some("String" | "PathBuf" | "std::path::PathBuf")
        ) =>
        {
            Some(InputField::Str)
        }
        Shape::Vec(e) if u8s(e) => Some(InputField::Bytes),
        Shape::Arc(i) if matches!(Shape::of(i), Shape::Vec(e) if u8s(e)) => {
            Some(InputField::SharedBytes)
        }
        // Behind a plain `&` too, as shared bytes are read anywhere.
        _ if matches!(Shape::of(unref(t)), Shape::Arc(i) if Shape::of(i) == Shape::DynBytes) => {
            Some(InputField::SharedBytes)
        }
        _ => None,
    }
}

/// Whether a variant's fields are all something JS can give, several of
/// them (a tuple): such a variant is in the input enum's type, and refused
/// when passed, there being no one `value` to build it from.
fn input_tuple(f: &syn::FieldsUnnamed) -> bool {
    f.unnamed.len() > 1 && f.unnamed.iter().all(|x| input_field(&x.ty).is_some())
}

/// Whether JS can pass a value of payload enum `e`: every variant is unit,
/// carries one string, path or run of bytes (`Name(&'a str)`, `File(PathBuf)`,
/// `Binary(Arc<dyn AsRef<[u8]>>)`), or a tuple of them. Such an enum is an
/// argument type; its input form ([`enum_input`]) is the `{ type, value }` /
/// `{ type, bytes }` its union prints as, converted for the call.
pub fn is_enum_input(e: &syn::ItemEnum) -> bool {
    e.generics.type_params().next().is_none()
        && e.variants.iter().any(|v| {
            matches!(&v.fields, Fields::Unnamed(f) if f.unnamed.len() == 1 && input_field(&f.unnamed[0].ty).is_some())
        })
        && e.variants.iter().all(|v| match &v.fields {
            Fields::Unit => true,
            Fields::Unnamed(f) if f.unnamed.len() == 1 => input_field(&f.unnamed[0].ty).is_some(),
            Fields::Unnamed(f) => input_tuple(f),
            Fields::Named(_) => false,
        })
}

/// The [`is_enum_input`] enums of `files` that JS has somewhere to pass: an
/// argument of a function, or a list in an option object
/// (`Config::option_objects`). The rest would be types nothing takes.
pub fn input_enums<'f>(files: &'f [syn::File], config: &Config) -> Vec<&'f syn::ItemEnum> {
    let mut taken = BTreeSet::new();
    let take = |taken: &mut BTreeSet<String>, sig: &syn::Signature| {
        for i in &sig.inputs {
            if let syn::FnArg::Typed(t) = i {
                let mut ty = &*t.ty;
                while let Shape::Ref { inner, .. } = Shape::of(ty) {
                    ty = inner;
                }
                if let Some(n) = Shape::of(ty).last() {
                    taken.insert(n.to_string());
                }
            }
        }
    };
    for item in files.iter().flat_map(|f| &f.items) {
        match item {
            Item::Fn(f) if is_pub(&f.vis) => take(&mut taken, &f.sig),
            // A method of an enum is not emitted: a union is plain data.
            Item::Impl(imp)
                if !self_ident(imp).is_some_and(|i| {
                    crate::sources::find_enum(files, &i.to_string()).is_some()
                }) =>
            {
                imp.items.iter().for_each(|it| {
                    if let ImplItem::Fn(f) = it
                        && is_pub(&f.vis)
                    {
                        take(&mut taken, &f.sig);
                    }
                })
            }
            Item::Struct(st)
                if config
                    .option_objects
                    .contains(&st.ident.to_string().as_str()) =>
            {
                for f in &st.fields {
                    if let Some(e) = input_list(files, &f.ty) {
                        taken.insert(e.ident.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(|i| match i {
            Item::Enum(e)
                if is_pub(&e.vis) && is_enum_input(e) && taken.contains(&e.ident.to_string()) =>
            {
                Some(e)
            }
            _ => None,
        })
        .collect()
}

/// Whether `files` emit the input form of enum `name` ([`input_enums`]):
/// the one test of what a member may take as an input enum, so none names a
/// form that was never emitted. The shape is checked first, being cheaper.
pub fn is_input(files: &[syn::File], name: &str, config: &Config) -> bool {
    crate::sources::find_enum(files, name).is_some_and(is_enum_input)
        && input_enums(files, config).iter().any(|e| e.ident == name)
}

/// The input form of an [`is_enum_input`] enum, named as upstream for
/// [`namespaced`] to prefix: `{ type: 'name', value: 'Arial' }`,
/// `{ type: 'binary', bytes }` or `{ type: 'serif' }`, and the `to_upstream`
/// that converts it, borrowing its strings.
pub fn enum_input(
    e: &syn::ItemEnum,
    modules: &BTreeMap<String, String>,
    root: &TokenStream,
    config: &Config,
) -> TokenStream {
    let name = e.ident.to_string();
    let ident = format_ident!("{name}");
    let up = upstream_path(&name, modules, root);
    let lifetimes = e.generics.lifetimes().map(|_| quote!('_));
    let up_ty = quote!(#up<#(#lifetimes),*>);
    let doc = docs(&e.attrs);
    let tags: Vec<String> = e
        .variants
        .iter()
        .map(|v| lower_camel(&v.ident.to_string()))
        .collect();
    let one = |v: &syn::Variant| match &v.fields {
        Fields::Unnamed(f) if f.unnamed.len() == 1 => input_field(&f.unnamed[0].ty),
        _ => None,
    };
    // Only what `to_upstream` accepts: a variant carrying several values
    // always throws, so the type leaves it out.
    let ts = e
        .variants
        .iter()
        .zip(&tags)
        .filter(|(v, _)| matches!(v.fields, Fields::Unit) || one(v).is_some())
        .map(|(_, t)| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(" | ");
    let listed = |bytes: bool| -> Vec<String> {
        e.variants
            .iter()
            .zip(&tags)
            .filter(|(v, _)| {
                matches!(
                    (one(v), bytes),
                    (Some(InputField::BorrowedStr | InputField::Str), false)
                        | (Some(InputField::Bytes | InputField::SharedBytes), true)
                )
            })
            .map(|(_, t)| format!("`'{t}'`"))
            .collect()
    };
    let (strs, bytes) = (listed(false), listed(true));
    // The forms it is passed in as: the bytes one only when a variant
    // carries bytes, or the doc names a field the type does not have.
    let forms = [
        (!strs.is_empty(), "`{ type, value }`"),
        (!bytes.is_empty(), "`{ type, bytes }`"),
    ]
    .into_iter()
    .filter_map(|(has, form)| has.then_some(form))
    .collect::<Vec<_>>();
    let passed = (!forms.is_empty()).then(|| {
        let d = format!(" Passed in as {}.", forms.join(" or "));
        quote!(#[doc = ""] #[doc = #d])
    });
    let mut fields = Vec::new();
    if !strs.is_empty() {
        let d = format!(
            " The string the variant carries: required for {}, ignored otherwise.",
            strs.join(", ")
        );
        fields.push(quote!(#[doc = #d] pub value: Option<String>,));
    }
    if !bytes.is_empty() {
        let d = format!(
            " The bytes the variant carries, copied: required for {}, ignored otherwise.",
            bytes.join(", ")
        );
        fields.push(quote!(#[doc = #d] pub bytes: Option<Uint8Array>,));
    }
    let arms = e.variants.iter().zip(&tags).map(|(v, tag)| {
        let vid = &v.ident;
        let g = crate::cfg::attr(config, &v.attrs);
        let missing = |what: &str| format!("{name} '{tag}' needs {what}");
        let value = |m: String| {
            quote!(self.value.as_deref().ok_or_else(|| napi_support::thrown("InvalidArg", #m))?)
        };
        let bytes = |m: String| {
            quote!(self.bytes.as_deref().ok_or_else(|| napi_support::thrown("InvalidArg", #m))?.to_vec())
        };
        match (&v.fields, one(v)) {
            (Fields::Unit, _) => quote!(#g #tag => #up::#vid,),
            (_, Some(InputField::BorrowedStr)) => {
                let x = value(missing("a value"));
                quote!(#g #tag => #up::#vid(#x),)
            }
            (_, Some(InputField::Str)) => {
                let x = value(missing("a value"));
                quote!(#g #tag => #up::#vid(#x.into()),)
            }
            (_, Some(InputField::Bytes)) => {
                let x = bytes(missing("bytes"));
                quote!(#g #tag => #up::#vid(#x),)
            }
            (_, Some(InputField::SharedBytes)) => {
                let x = bytes(missing("bytes"));
                quote!(#g #tag => #up::#vid(std::sync::Arc::new(#x)),)
            }
            _ => {
                // Ungated: it names no variant, and says the same everywhere.
                let m = format!("{name} '{tag}' cannot be passed in: it carries several values");
                quote!(#tag => return Err(napi_support::thrown("InvalidArg", #m)),)
            }
        }
    });
    let invalid = format!("invalid {name} type: {{:?}}");
    quote! {
        #(#doc)*
        #passed
        #[napi(object, object_to_js = false)]
        pub struct #ident {
            #[doc = " The variant."]
            #[napi(ts_type = #ts)]
            pub r#type: String,
            #(#fields)*
        }

        impl #ident {
            #[doc = " The upstream value, borrowing this one's strings."]
            fn to_upstream(&self) -> Result<#up_ty, String> {
                Ok(match self.r#type.as_str() {
                    #(#arms)*
                    t => return Err(napi_support::thrown("InvalidArg", format!(#invalid, t))),
                })
            }
        }
    }
}

/// The input enum a field `&'a [E<'a>]` lists, when `E` is one
/// ([`is_enum_input`]).
fn input_list<'f>(files: &'f [syn::File], ty: &syn::Type) -> Option<&'f syn::ItemEnum> {
    let Shape::Ref {
        mutable: false,
        inner,
        ..
    } = Shape::of(ty)
    else {
        return None;
    };
    let Shape::Slice(elem) = Shape::of(inner) else {
        return None;
    };
    let name = Shape::of(elem).last()?.to_string();
    crate::sources::find_enum(files, &name).filter(|e| is_enum_input(e))
}

/// Whether option object `ty` has a field listing input enums: its
/// `to_upstream` then borrows what its `lists` converted.
pub fn has_input_lists(files: &[syn::File], ty: &str) -> bool {
    files.iter().flat_map(|f| &f.items).any(|i| match i {
        Item::Struct(st) if st.ident == ty && is_pub(&st.vis) => st
            .fields
            .iter()
            .any(|f| is_pub(&f.vis) && input_list(files, &f.ty).is_some()),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One table: a kind that both a getter and a method can return has the
    /// same JS type at both, whatever expression each uses to get there.
    #[test]
    fn getter_and_method_agree() {
        let (v, a) = (Vocab::default(), quote!(x));
        for js in [
            Js::Bool,
            Js::I32,
            Js::I16,
            Js::F32,
            Js::F64,
            Js::U8,
            Js::U16,
            Js::U32,
            Js::Count,
            Js::Str,
            Js::Scalar,
            Js::Class("C".into()),
            Js::OptClass("C".into()),
            Js::IntNewtype("N".into()),
            Js::Value("V".into()),
            Js::ValueList("V".into()),
            Js::OptValue("V".into()),
            Js::Object("O".into()),
            Js::ObjectList("O".into()),
        ] {
            let ty =
                |site| js_conv(&js, &syn::parse_quote!(T), &v, &a, site).map(|c| c.ty.to_string());
            assert_eq!(
                ty(Site::Getter),
                ty(Site::Method),
                "{js:?}: getter and method disagree"
            );
        }
    }

    /// A free function is renamed where it is defined and called, a method
    /// of the same name is not (#260).
    #[test]
    fn a_method_keeps_its_name() {
        let renames = BTreeMap::from([("boom".to_string(), format_ident!("ToyBoom"))]);
        let mut named = BTreeSet::new();
        let free = rename_idents(
            quote!(
                pub fn boom() {}
            ),
            &renames,
            &mut named,
        );
        assert_eq!(
            free.to_string(),
            quote!(
                pub fn ToyBoom() {}
            )
            .to_string()
        );
        let method = rename_idents(
            quote!(impl Canvas { pub fn boom(&self) { boom() } }),
            &renames,
            &mut named,
        );
        assert_eq!(
            method.to_string(),
            quote!(impl Canvas { pub fn boom(&self) { ToyBoom() } }).to_string()
        );
    }
}
