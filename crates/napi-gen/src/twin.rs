//! Async twins (ADR 0001; ADR 0002, Q28): for each member
//! `Config::asynchronous` lists, `<name>Async(...args, signal?)`, which
//! converts its arguments on the main thread, runs the call on a worker
//! thread and returns a promise (`napi_support::spawn`).
//!
//! What crosses to the worker is owned and `Send`: bytes and strings are
//! copied, an option object becomes its upstream struct, a class behind an
//! `Arc` or a projection is cloned, a `Copy` class copied. A class held by
//! value and borrowed (`&T` of an owned class, `&mut T` of any, a `&mut`
//! view) is leased (`napi_support::Slot::lease`): its value goes with the
//! call, every use of the instance meanwhile throws `Leased`, and it is back
//! however the call ends. The result is made JS back on the main thread,
//! and an error rejects with the `code` and `message` the sync call throws.

use std::collections::BTreeMap;

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{FnArg, ReturnType, Type};

use crate::class::{Classes, Holding, Prim, Recv, args, deref, is, is_unit, receiver, segs};
use crate::config;
use crate::vocab::{Vocab, lower_camel, ty_str};

/// Whether `Config::asynchronous` lists `name` (`Type::name` for a member)
/// of the source at `root`.
pub fn listed(root: &str, name: &str) -> bool {
    config()
        .asynchronous
        .iter()
        .any(|(r, n)| *r == root && *n == name)
}

impl Classes<'_> {
    /// One argument of a twin: its parameter, the statement converting it
    /// on the main thread into an owned `Send` value, and the expression the
    /// worker passes. An `Err` says why it cannot cross.
    fn twin_arg(
        &self,
        ctx: usize,
        this: Option<usize>,
        pat: &Ident,
        ty: &Type,
        bounds: &BTreeMap<String, String>,
    ) -> Result<(TokenStream, Option<TokenStream>, TokenStream), String> {
        let name = lower_camel(&pat.to_string());
        let tmp = format_ident!("__{pat}");
        let (r, inner) = deref(ty);
        let segs = segs(inner);
        if let Some([p]) = segs.as_deref()
            && bounds.contains_key(p)
        {
            // A path or a string, as the sync call takes it: owned already.
            return Ok((quote!(#pat: String), None, quote!(#pat)));
        }
        if let (Some(false), Type::Slice(s)) = (r, inner)
            && Prim::of(&s.elem) == Some(Prim::U8)
        {
            return Ok((
                quote!(#pat: Uint8Array),
                Some(quote!(let #tmp = #pat.to_vec();)),
                quote!(&#tmp),
            ));
        }
        if r == Some(false) && matches!(segs.as_deref(), Some([s]) if s == "str") {
            return Ok((quote!(#pat: String), None, quote!(&#pat)));
        }
        if r.is_none() && is(inner, "Vec", 1) && Prim::of(args(inner)[0]) == Some(Prim::U8) {
            return Ok((
                quote!(#pat: Uint8Array),
                Some(quote!(let #tmp = #pat.to_vec();)),
                quote!(#tmp),
            ));
        }
        if r.is_none()
            && let Some(p) = Prim::of(inner)
        {
            let js = p.js();
            let same = quote!(#pat);
            let conv = p.js_in(&same, &name);
            let pre = (conv.to_string() != same.to_string()).then(|| quote!(let #pat = #conv;));
            return Ok((quote!(#pat: #js), pre, quote!(#pat)));
        }
        if let Some(c) = self.class_of(ctx, this, inner) {
            let id = self.ident(c);
            use Holding::*;
            let (mutable, pre, fwd) = match (r, self.classes[c].holding) {
                (None, Value) => (false, quote!(let #tmp = *#pat.__ref()?;), quote!(#tmp)),
                (Some(false), Value) => (false, quote!(let #tmp = *#pat.__ref()?;), quote!(&#tmp)),
                (Some(false), Shared | SharedCow) => (
                    false,
                    quote!(let #tmp = #pat.__ref()?.clone();),
                    quote!(&#tmp),
                ),
                (Some(false), Projected) => (
                    false,
                    quote!(let #tmp = #pat.__ref()?.clone();),
                    quote!(#tmp.get()),
                ),
                // Held by value and borrowed: lent to the call.
                (Some(false), Owned) => (true, quote!(let #tmp = #pat.__lease()?;), quote!(&#tmp)),
                (Some(true), Value | Owned) => (
                    true,
                    quote!(let mut #tmp = #pat.__lease()?;),
                    quote!(&mut *#tmp),
                ),
                (Some(true), SharedCow) => (
                    true,
                    quote!(let mut #tmp = #pat.__lease()?;),
                    quote!(std::sync::Arc::make_mut(&mut *#tmp)),
                ),
                _ => return Err(format!("argument `{pat}` cannot cross to a worker thread")),
            };
            let param = if mutable {
                quote!(#pat: &mut #id)
            } else {
                quote!(#pat: &#id)
            };
            return Ok((param, Some(pre), fwd));
        }
        if r == Some(false)
            && let Some(u) = self.union_of(ctx, inner)
        {
            // A projection of its own: owned already.
            let (param, pre, fwd) = self.union_arg(u, pat);
            return Ok((param, pre, fwd));
        }
        let missing = || format!("argument `{pat}` has no JS form");
        let segs = segs.ok_or_else(missing)?;
        let last = segs.last().ok_or_else(missing)?;
        if r == Some(true)
            && let Some(v) = config().views.iter().find(|v| v.ty == last)
        {
            let mut class_segs = segs.clone();
            *class_segs.last_mut().ok_or_else(missing)? = v.class.to_string();
            let fake: Type = syn::parse_str(&class_segs.join("::")).map_err(|e| e.to_string())?;
            let c = self.class_of(ctx, this, &fake).ok_or_else(missing)?;
            let (id, via) = (self.ident(c), format_ident!("{}", v.via));
            let place = match self.classes[c].holding {
                Holding::Owned | Holding::Value => quote!((*#tmp)),
                Holding::SharedCow => quote!(std::sync::Arc::make_mut(&mut *#tmp)),
                Holding::Shared | Holding::Projected => return Err(missing()),
            };
            return Ok((
                quote!(#pat: &mut #id),
                Some(quote!(let mut #tmp = #pat.__lease()?;)),
                quote!(&mut #place.#via()),
            ));
        }
        if r == Some(false) && config().option_objects.contains(&last.as_str()) {
            let o = format_ident!("{last}");
            return Ok((
                quote!(#pat: Option<#o>),
                Some(quote!(let #tmp = #pat.unwrap_or_default().to_upstream()?;)),
                quote!(&#tmp),
            ));
        }
        Err(missing())
    }

    /// The async twin of a method of class `this` (`recv` set), a static of
    /// it, or a free function (`this` None), named `<name>_async`. `free` is
    /// a free function's JS name and namespace. Panics when no twin can be made: listing one
    /// is a decision, and a silent miss would hide it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn twin(
        &self,
        ctx: usize,
        this: Option<usize>,
        sig: &syn::Signature,
        gate: Option<(TokenStream, TokenStream)>,
        free: Option<(&str, &str)>,
        vocab: &Vocab,
    ) -> TokenStream {
        let name = sig.ident.to_string();
        let label = match this {
            Some(c) => format!("{}::{name}", self.classes[c].name),
            None => name.clone(),
        };
        fn fail_at(label: &str, why: &str) -> ! {
            panic!("Config::asynchronous lists {label}, which gets no async twin: {why}")
        }
        let fail = |why: &str| -> ! { fail_at(&label, why) };
        let recv = receiver(sig).unwrap_or_else(|| fail("its receiver has no JS form"));
        if this.is_some() && recv == Recv::None && name == "new" {
            fail("a constructor cannot be async: list a factory");
        }
        let holding = this.map(|c| self.classes[c].holding);
        let ident = &sig.ident;
        // The receiver, made `Send` on the main thread, and the call on it.
        let (me, self_pre, call) = match (recv, holding) {
            (Recv::None, _) => {
                let path = match this {
                    Some(c) => {
                        let p = self.path(self.classes[c].source, &self.classes[c].name);
                        quote!(<#p>)
                    }
                    None => self.sources[ctx].root.parse().expect("a root path"),
                };
                (None, None, quote!(#path::#ident))
            }
            (Recv::Ref, Some(Holding::Projected)) => (
                Some(quote!(&self)),
                Some(quote!(let __self = self.__ref()?.clone();)),
                quote!(__self.get().#ident),
            ),
            (Recv::Ref, Some(Holding::Shared | Holding::SharedCow)) => (
                Some(quote!(&self)),
                Some(quote!(let __self = self.__ref()?.clone();)),
                quote!(__self.#ident),
            ),
            (Recv::Ref | Recv::Value, Some(Holding::Value)) => (
                Some(quote!(&self)),
                Some(quote!(let __self = *self.__ref()?;)),
                quote!(__self.#ident),
            ),
            (Recv::Ref, Some(Holding::Owned)) => (
                Some(quote!(&mut self)),
                Some(quote!(let __self = self.__lease()?;)),
                quote!(__self.#ident),
            ),
            (Recv::Mut, Some(Holding::Value | Holding::Owned)) => (
                Some(quote!(&mut self)),
                Some(quote!(let mut __self = self.__lease()?;)),
                quote!(__self.#ident),
            ),
            (Recv::Mut, Some(Holding::SharedCow)) => (
                Some(quote!(&mut self)),
                Some(quote!(let mut __self = self.__lease()?;)),
                quote!(std::sync::Arc::make_mut(&mut *__self).#ident),
            ),
            (Recv::Value, Some(Holding::Owned)) => (
                Some(quote!(&mut self)),
                Some(quote!(let __self = self.__take()?;)),
                quote!(__self.#ident),
            ),
            _ => fail("its receiver cannot cross to a worker thread"),
        };

        let bounds: BTreeMap<String, String> = sig
            .generics
            .type_params()
            .map(|p| (p.ident.to_string(), quote!(#p).to_string().replace(' ', "")))
            .collect();
        let (mut params, mut pre, mut fwd) = (Vec::new(), Vec::new(), Vec::new());
        // What the call leases: what the doc names, and the locals holding
        // them, which `spawn` keeps a hold on.
        let leases = |pre: &Option<TokenStream>| {
            pre.as_ref()
                .is_some_and(|p| p.to_string().contains("__lease"))
        };
        let (mut lent, mut held) = (Vec::new(), Vec::new());
        for input in sig.inputs.iter().skip(usize::from(recv != Recv::None)) {
            let FnArg::Typed(t) = input else { continue };
            let syn::Pat::Ident(p) = &*t.pat else {
                fail("an argument is a pattern")
            };
            let pat = &p.ident;
            let (param, p, f) = self
                .twin_arg(ctx, this, pat, &t.ty, &bounds)
                .unwrap_or_else(|why| fail(&why));
            if leases(&p) {
                lent.push(format!("`{}`", lower_camel(&pat.to_string())));
                held.push(format_ident!("__{pat}"));
            }
            params.push(param);
            pre.extend(p);
            fwd.push(f);
        }
        if leases(&self_pre) {
            lent.insert(0, "this instance".to_string());
            held.push(format_ident!("__self"));
        }

        // Result<T, E>, then Option<T>, then T, as the sync call reads them.
        let out = match &sig.output {
            ReturnType::Default => syn::parse_quote!(()),
            ReturnType::Type(_, t) => (**t).clone(),
        };
        let (out, err) = if is(&out, "Result", 2) || is(&out, "Result", 1) {
            let a = args(&out);
            (a[0].clone(), Some(a.get(1).copied().cloned()))
        } else {
            (out, None)
        };
        let (out, optional) = if is(&out, "Option", 1) {
            (args(&out)[0].clone(), true)
        } else {
            (out, false)
        };
        if deref(&out).0.is_some() {
            fail("it returns a borrow, which cannot leave the worker thread");
        }
        let (js_ty, conv) = if is_unit(&out) {
            if optional {
                (quote!(Option<bool>), quote!(__v.map(|_| true)))
            } else {
                (quote!(()), quote!(__v))
            }
        } else {
            // Never the class's own factory: napi builds a class with a
            // shared name from the constructor it registered, which only
            // the sync factory is called on.
            let ret = self
                .ret(ctx, this, &out, false, false, vocab)
                .unwrap_or_else(|_| fail(&format!("it returns {}", ty_str(&out))));
            let (ty, conv) = (ret.ty, ret.conv);
            if optional {
                (quote!(Option<#ty>), quote!(__v.map(|__v| #conv)))
            } else {
                (ty, conv)
            }
        };
        let mut run = quote!(napi_support::contained(#label, || #call(#(#fwd),*)).map_err(napi_support::coded));
        if let Some(e) = &err {
            let thrown = self.thrown(ctx, e.as_ref());
            run = quote!(#run?.map_err(#thrown));
        }
        // The receiver last, as in the sync call: a lease is taken only once
        // every argument converted.
        let mut body = quote! {
            #(#pre)*
            #self_pre
            napi_support::spawn(env, vec![#(#held.hold()),*], move || #run, |__v| Ok(#conv), signal)
        };
        let mut cfg_attr = None;
        if let Some((p, n)) = gate {
            let missing = format!(
                "{}Async is not available on this target: its upstream feature is off here",
                lower_camel(&name)
            );
            cfg_attr = Some(quote!(#[cfg_attr(#n, allow(unused_variables, unused_mut))]));
            body = quote! {
                #[cfg(#p)]
                { #body }
                #[cfg(#n)]
                { Err(napi_support::thrown("GenericFailure", #missing)) }
            };
        }
        let twin = format_ident!("{name}_async");
        let js = lower_camel(&name);
        let mut doc = vec![
            format!(" `{js}` on a worker thread. The arguments are read now, the call runs"),
            " off the main thread, and the promise resolves with its result or rejects".into(),
            format!(" as `{js}` throws. `signal` drops the call while it is still queued."),
        ];
        if !lent.is_empty() {
            doc.push(format!(
                " Until it settles, {} is in use: any other use throws `Leased`.",
                lent.join(" and ")
            ));
        }
        let doc = doc.iter().map(|l| quote!(#[doc = #l]));
        // `Promise<void>`, where napi would say `Promise<undefined>`, and the
        // sync call's `boolean | null` for `Option<()>`.
        let ts = match (is_unit(&out), optional) {
            (true, false) => Some(quote!(ts_return_type = "Promise<void>")),
            (true, true) => Some(quote!(ts_return_type = "Promise<boolean | null>")),
            _ => None,
        };
        let napi = match free {
            Some((js, ns)) => {
                let ts = ts.map(|t| quote!(, #t));
                quote!(#[napi(js_name = #js, namespace = #ns #ts)])
            }
            None => match ts {
                Some(ts) => quote!(#[napi(#ts)]),
                None => quote!(#[napi]),
            },
        };
        let params = me.into_iter().chain(params);
        quote! {
            #(#doc)*
            #napi
            #cfg_attr
            #[allow(clippy::redundant_closure)]
            pub fn #twin(
                #(#params,)*
                env: Env,
                #[napi(ts_arg_type = "AbortSignal | undefined | null")]
                signal: Option<napi_support::Signal>,
            ) -> Result<PromiseRaw<'static, #js_ty>, String> {
                #body
            }
        }
    }
}
