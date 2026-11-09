//! Projections: values only ever borrowed from an `Arc`-held owner (the
//! nodes of a tree), mirrored as classes that keep the owner alive and find
//! the value again on each call (`napi_support::Projection`).
//!
//! `Config::projections` lists them. A struct becomes a class of
//! [`Holding::Projected`]. An enum whose every variant carries one of those
//! structs (`V(Box<S>)` or `V(S)`) becomes a [`Union`]: one `{ kind, value }`
//! object per variant, `kind` the variant's name in camelCase and `value` the
//! projected class. A method of a projected class, or of a class behind an
//! `Arc`, that returns `&T`, `Option<&T>`, `&[T]` or `&[Arc<T>]` of either
//! is a step of a projection from its receiver: the method is called again,
//! on the receiver found again, each time the result is used.

use convert_case::{Case, Casing};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{Fields, Type};

use crate::class::{
    Args, Class, Classes, Holding, Source, args, deref, find_enum, find_struct, is, segs,
};
use crate::emit::stepped;
use crate::sources::derives;
use crate::vocab::{Js, Reason, Vocab, lower_camel, ty_str};
use crate::{config, namespace_of};

/// A `Config::projections` enum, mirrored as a union of projections.
pub struct Union {
    pub source: usize,
    pub name: String,
    /// Each variant, the class it carries, and whether in a `Box`.
    variants: Vec<(Ident, usize, bool)>,
}

/// Adds the structs `Config::projections` lists to `classes`, and returns
/// its enums as unions of them.
pub(crate) fn projections(sources: &[Source], classes: &mut Vec<Class>) -> Vec<Union> {
    let source_of = |root: &str| {
        sources
            .iter()
            .position(|s| s.root == root)
            .unwrap_or_else(|| panic!("Config::projections: no source with root {root}"))
    };
    for (root, name) in config().projections {
        let source = source_of(root);
        if find_struct(sources[source].files, name).is_some() {
            classes.push(Class {
                source,
                name: name.to_string(),
                holding: Holding::Projected,
                plain: Vec::new(),
                shared_name: false,
                of: None,
            });
        }
    }
    let mut unions = Vec::new();
    for (root, name) in config().projections {
        let source = source_of(root);
        if find_struct(sources[source].files, name).is_some() {
            continue;
        }
        let e = find_enum(sources[source].files, name)
            .unwrap_or_else(|| panic!("upstream: no `pub struct` or `pub enum {name}` in {root}"));
        assert!(
            e.variants.len() >= 2 && !e.attrs.iter().any(|a| a.path().is_ident("non_exhaustive")),
            "{root}::{name}: a union needs two variants or more, all known"
        );
        let mut variants = Vec::new();
        for v in &e.variants {
            let carried = match &v.fields {
                Fields::Unnamed(f) if f.unnamed.len() == 1 => &f.unnamed[0].ty,
                _ => panic!(
                    "{root}::{name}::{}: a union variant carries one value",
                    v.ident
                ),
            };
            let boxed = is(carried, "Box", 1);
            let ty = if boxed { args(carried)[0] } else { carried };
            let carried = segs(ty).and_then(|s| s.last().cloned());
            let c = classes
                .iter()
                .position(|c| {
                    c.source == source
                        && c.holding == Holding::Projected
                        && Some(&c.name) == carried.as_ref()
                })
                .unwrap_or_else(|| {
                    panic!("{root}::{name}::{}: carries no projected struct", v.ident)
                });
            assert!(
                classes[c].of.is_none(),
                "{}: carried by two unions",
                classes[c].name
            );
            classes[c].of = Some(unions.len());
            variants.push((v.ident.clone(), c, boxed));
        }
        unions.push(Union {
            source,
            name: name.to_string(),
            variants,
        });
    }
    unions
}

impl Classes<'_> {
    /// The union `ty` names from source `ctx`.
    pub(crate) fn union_of(&self, ctx: usize, ty: &Type) -> Option<usize> {
        let segs = segs(ty)?;
        let name = segs.last()?;
        let has = |s: usize| {
            self.unions
                .iter()
                .position(|u| u.source == s && u.name == *name)
        };
        has(self.resolve(ctx, &segs, |s| has(s).is_some())?)
    }

    fn helper(&self, u: usize, what: &str) -> Ident {
        format_ident!("{}_{what}", self.unions[u].name.to_case(Case::Snake))
    }

    /// The union's JS type: `EitherN` of its variants' objects.
    fn union_ty(&self, u: usize) -> TokenStream {
        let un = &self.unions[u];
        let objs = un
            .variants
            .iter()
            .map(|(v, _, _)| format_ident!("{}{v}", un.name));
        let either = match un.variants.len() {
            2 => format_ident!("Either"),
            n => format_ident!("Either{n}"),
        };
        quote!(#either<#(#objs),*>)
    }

    /// An argument `&E` of union `u`: the `{ kind, value }` object, whose
    /// value was reached as a variant of `E`.
    pub(crate) fn union_arg(
        &self,
        u: usize,
        pat: &Ident,
    ) -> (TokenStream, Option<TokenStream>, TokenStream) {
        let un = &self.unions[u];
        let ns =
            namespace_of(self.sources[un.source].root).map_or(String::new(), |n| format!("{n}."));
        let ts: Vec<String> = un
            .variants
            .iter()
            .map(|(v, _, _)| format!("{ns}{}{v}", un.name))
            .collect();
        let ts = ts.join(" | ");
        let (from, tmp) = (self.helper(u, "from_js"), format_ident!("__{pat}"));
        (
            quote!(#[napi(ts_arg_type = #ts)] #pat: Object),
            Some(quote!(let #tmp = #from(&#pat)?;)),
            quote!(#tmp.get()),
        )
    }

    /// A method of a class of holding `h` returning `out` (`optional` when
    /// it was an `Option`): when `out` borrows a projected class or union,
    /// or a value `Vocab::steps_into`, its JS type, the statement binding the
    /// receiver's projection `__b` (it may throw `Disposed`), and the value,
    /// made by a step from `__b`. `None` when it borrows none of them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn projected(
        &self,
        h: Holding,
        ctx: usize,
        m: &Ident,
        out: &Type,
        optional: bool,
        a: &Args,
        vocab: &Vocab,
    ) -> Option<Result<(TokenStream, TokenStream, TokenStream), Reason>> {
        let (r, inner) = deref(out);
        if r != Some(false) {
            return None;
        }
        let (list, elem) = match inner {
            Type::Slice(s) => (true, &*s.elem),
            t => (false, t),
        };
        // An `Arc` in a table (`&[Arc<T>]`) is stepped through to the `T`.
        let arc = is(elem, "Arc", 1);
        let elem = if arc { args(elem)[0] } else { elem };
        let through = arc.then(|| quote!(.map(|__a| &**__a)));
        let (ty, conv) = if let Some(c) = self
            .class_of(ctx, None, elem)
            .filter(|&c| self.classes[c].holding == Holding::Projected)
        {
            let id = self.ident(c);
            (quote!(#id), quote!(#id::__proj))
        } else if let Some(u) = self.union_of(ctx, elem) {
            let f = self.helper(u, "to_js");
            (self.union_ty(u), quote!(#f))
        } else {
            // A read-only class or a union carrying a value, borrowed from
            // a receiver that is never written: one held by a projection or
            // a read-only `Arc`. Elsewhere it stays a copy: a projection
            // into a copy-on-write `Arc` would keep it shared, and the next
            // write would copy the whole of it.
            let js = vocab.classify(&ty_str(elem))?;
            let stepped = stepped(&js, vocab).filter(|_| !matches!(js, Js::Owned(_)))?;
            if !matches!(h, Holding::Projected | Holding::Shared) {
                return None;
            }
            stepped
        };
        let base = match h {
            Holding::Projected => quote!(self.__ref()?.clone()),
            Holding::Shared | Holding::SharedCow => {
                quote!(napi_support::Projection::of(self.__ref()?.clone()))
            }
            Holding::Value | Holding::Owned => return Some(Err(Reason::Returns)),
        };
        // The step calls the method again, so the arguments move into it:
        // values only, and none for a list.
        if a.params.iter().any(|p| p.to_string().contains('&')) || (list && !a.params.is_empty()) {
            return Some(Err(Reason::Arg));
        }
        let fwd = &a.fwd;
        let bind = quote!(let __b = #base;);
        let (ty, value) = if list {
            let value = quote!({
                (0..__b.get().#m().len())
                    .filter_map(|__i| __b.then(move |__s| __s.#m().get(__i) #through))
                    .map(#conv)
                    .collect()
            });
            (quote!(Vec<#ty>), value)
        } else if optional {
            (
                quote!(Option<#ty>),
                quote!(__b.then(move |__s| __s.#m(#(#fwd),*) #through).map(#conv)),
            )
        } else {
            let p = quote!(__b.then(move |__s| Some(__s.#m(#(#fwd),*)) #through).expect("a borrow resolves"));
            (ty, quote!(#conv(#p)))
        };
        Some(Ok((ty, bind, value)))
    }

    /// A projected class's struct and helpers: `__proj` from a projection,
    /// and `__from`/`wrap`/`From<&T>` from a value of its own, for what
    /// hands out a copy.
    pub(crate) fn projected_class(&self, c: usize, up: &TokenStream) -> (TokenStream, TokenStream) {
        let class = &self.classes[c];
        let id = self.ident(c);
        let (of, of_none) = match class.of {
            Some(u) => {
                let e = self.path(self.unions[u].source, &self.unions[u].name);
                (
                    quote!(, of: Option<napi_support::Projection<#e>>),
                    quote!(, of: None),
                )
            }
            None => (quote!(), quote!()),
        };
        let from = derives(self.sources[class.source].files, &class.name, "Clone").then(|| {
            quote! {
                impl From<&#up> for #id {
                    fn from(v: &#up) -> Self { Self::__from(v.clone()) }
                }
            }
        });
        let def = quote! {
            pub struct #id { inner: napi_support::Slot<napi_support::Projection<#up>> #of }
            #from
        };
        let helpers = quote! {
            fn __proj(inner: napi_support::Projection<#up>) -> Self {
                Self { inner: napi_support::Slot::new(inner) #of_none }
            }
            fn __from(inner: #up) -> Self { Self::__proj(napi_support::Projection::owned(inner)) }
            fn wrap(inner: #up) -> Self { Self::__from(inner) }
        };
        (def, helpers)
    }

    /// Each union's variant objects and its two helpers, by source:
    /// `<e>_to_js` from a projection of the enum, and `<e>_from_js` back
    /// from the object.
    pub fn union_code(&self) -> Vec<(usize, TokenStream)> {
        (0..self.unions.len())
            .map(|u| {
                let un = &self.unions[u];
                let e = self.path(un.source, &un.name);
                let (to, from, ty) = (self.helper(u, "to_js"), self.helper(u, "from_js"), self.union_ty(u));
                let either = match un.variants.len() {
                    2 => format_ident!("Either"),
                    n => format_ident!("Either{n}"),
                };
                let mut code = TokenStream::new();
                let (mut to_arms, mut from_arms) = (Vec::new(), Vec::new());
                for (i, (v, c, boxed)) in un.variants.iter().enumerate() {
                    let (obj, class) = (format_ident!("{}{v}", un.name), self.ident(*c));
                    let kind = lower_camel(&v.to_string());
                    let (ts, doc) = (format!("'{kind}'"), format!(" `{}::{v}`.", un.name));
                    let letter = format_ident!("{}", char::from(b'A' + u8::try_from(i).expect("26 variants at most")));
                    let unboxed = if *boxed { quote!(&**__v) } else { quote!(__v) };
                    code.extend(quote! {
                        #[doc = #doc]
                        #[napi(object, object_from_js = false)]
                        pub struct #obj {
                            #[doc = " Discriminant. Narrow on this."]
                            #[napi(ts_type = #ts)]
                            pub kind: String,
                            pub value: #class,
                        }
                    });
                    to_arms.push(quote! {
                        #e::#v(_) => #either::#letter(#obj {
                            kind: #kind.to_string(),
                            value: #class {
                                inner: napi_support::Slot::new(
                                    p.then(|__e| match __e { #e::#v(__v) => Some(#unboxed), _ => None })
                                        .expect("the variant matched"),
                                ),
                                of: Some(p.clone()),
                            },
                        }),
                    });
                    from_arms.push(quote! {
                        #kind => {
                            let __c = o.get_named_property_unchecked::<ClassInstance<#class>>("value").map_err(napi_support::coded)?;
                            __c.__ref()?;
                            __c.of.clone()
                        }
                    });
                }
                let why = format!("not a value reached as a variant of {}", un.name);
                code.extend(quote! {
                    fn #to(p: napi_support::Projection<#e>) -> #ty {
                        match p.get() { #(#to_arms)* }
                    }

                    fn #from(o: &Object) -> Result<napi_support::Projection<#e>, String> {
                        use napi::bindgen_prelude::JsObjectValue as _;
                        let kind: String = o.get_named_property_unchecked("kind").map_err(napi_support::coded)?;
                        let of = match kind.as_str() { #(#from_arms)* _ => None };
                        of.ok_or_else(|| napi_support::thrown("InvalidArg", format!("{}: kind {kind:?}", #why)))
                    }
                });
                (un.source, code)
            })
            .collect()
    }
}
