//! `dispose()`, `disposed` and `[Symbol.dispose]` for every class (ADR 0001,
//! "These stay generic").
//!
//! [`crate::class`] holds each class's value in a `napi_support::Slot` and
//! emits its `dispose()` itself. The other classes of the namespaced surface
//! (the read-only views and handles the passes produce) are put in one
//! afterwards, here, by [`slotted`]. [`aliases`] then reads every class with
//! a `dispose()` off the finished file and writes the two things `using`
//! needs: `[Symbol.dispose]` on each prototype, set when the module loads,
//! and its declaration, a `.d.ts` header merged into each class.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::visit_mut::VisitMut;
use syn::{Fields, ImplItem, Item, ReturnType};

use crate::emit::{class_of, napi_args};
use crate::sources::{inherent_impls, self_ident};
use crate::vocab::ty_str;

/// A class [`slotted`] puts in a slot: its field and JS name.
struct Target {
    field: Ident,
    js: String,
}

/// Puts the value of every namespaced class not in a `napi_support::Slot`
/// yet in one, and gives the class `dispose()` and `disposed`. Such a class
/// has one field: it is built with that field, read through `self.<field>`
/// in its `#[napi]` methods, and nowhere else. Each of those methods takes
/// the value from the slot first, so it throws `Disposed` once it is empty,
/// and returns `Result<T, String>` (napi unwraps it, so the `.d.ts` does not
/// change).
pub fn slotted(file: &mut syn::File) {
    let mut targets: BTreeMap<String, Target> = BTreeMap::new();
    let mut extra = TokenStream::new();
    for item in &mut file.items {
        let Some((id, js, Some(ns))) = class_of(item).map(|(i, j, n)| (i.clone(), j, n)) else {
            continue;
        };
        let Item::Struct(s) = item else { continue };
        let Fields::Named(fields) = &mut s.fields else {
            panic!("{id}: a class with named fields");
        };
        if fields
            .named
            .iter()
            .any(|f| ty_str(&f.ty).starts_with("napi_support::Slot<"))
        {
            continue;
        }
        assert!(
            fields.named.len() == 1,
            "{id}: a class with one field, which dispose() puts in a slot"
        );
        let f = &mut fields.named[0];
        let (field, ty) = (f.ident.clone().expect("a named field"), &f.ty);
        f.ty = syn::parse_quote!(napi_support::Slot<#ty>);
        extra.extend(quote! {
            #[napi(namespace = #ns)]
            impl #id {
                #[doc = " Drops this instance's value now, not at garbage collection: every later"]
                #[doc = " call throws `Disposed`. Also `[Symbol.dispose]` where the runtime has"]
                #[doc = " the symbol, for `using`. Harmless twice."]
                #[napi]
                pub fn dispose(&mut self) -> Result<(), String> {
                    self.#field.dispose(#js)
                }
                #[doc = " Whether `dispose()` has been called."]
                #[napi(getter)]
                pub fn disposed(&self) -> bool {
                    self.#field.disposed()
                }
            }
        });
        targets.insert(id.to_string(), Target { field, js });
    }
    for item in &mut file.items {
        let this = match item {
            Item::Impl(imp) => self_ident(imp).map(|i| i.to_string()),
            _ => None,
        };
        if let (Some(this), Item::Impl(imp)) = (&this, &mut *item)
            && let Some(t) = targets.get(this)
        {
            for it in &mut imp.items {
                let ImplItem::Fn(f) = it else { continue };
                assert!(
                    !["dispose", "disposed"].contains(&f.sig.ident.to_string().as_str()),
                    "{this}: `{}` is the generated dispose()",
                    f.sig.ident
                );
                guard(f, t, napi_args(&f.attrs).is_some(), this);
            }
        }
        Built {
            targets: &targets,
            this: this.as_deref(),
        }
        .visit_item_mut(item);
    }
    let extra: syn::File = syn::parse2(extra).expect("dispose impls that parse");
    file.items.extend(extra.items);
}

/// A method of a slotted class: a `#[napi]` one takes the value from the
/// slot first, and reads `self.<field>` from there.
fn guard(f: &mut syn::ImplItemFn, t: &Target, napi: bool, class: &str) {
    let mutable = match f.sig.inputs.first() {
        Some(syn::FnArg::Receiver(r)) => match &r.kind {
            syn::ReceiverKind::Reference(_, _, m) => m.is_some(),
            _ => return,
        },
        _ => return,
    };
    let local = format_ident!("__{}", t.field);
    let mut reads = Reads {
        field: &t.field,
        local: &local,
        shared: !mutable,
        found: false,
    };
    reads.visit_block_mut(&mut f.block);
    if !napi {
        assert!(
            !reads.found,
            "{class}::{}: only a #[napi] method may read the slotted field",
            f.sig.ident
        );
        return;
    }
    // Checked even where the value is not read (a constant getter): every
    // member of a disposed instance throws.
    let (field, js) = (&t.field, &t.js);
    let take = match (reads.found, mutable) {
        (true, true) => quote!(let #local = self.#field.get_mut(#js)?;),
        (true, false) => quote!(let #local = self.#field.get(#js)?;),
        (false, _) => quote!(self.#field.get(#js)?;),
    };
    let body = &f.block;
    let out = match &f.sig.output {
        ReturnType::Type(_, t) => Some((**t).clone()),
        ReturnType::Default => None,
    };
    let out_str = out.as_ref().map(ty_str).unwrap_or_default();
    let (block, ret): (syn::Block, syn::Type) =
        if out_str.starts_with("Result<") && out_str.ends_with(",String>") {
            let stmts = &body.stmts;
            (
                syn::parse_quote!({ #take #(#stmts)* }),
                out.expect("a Result"),
            )
        } else if out_str.starts_with("Result<") {
            // napi's `Result<T>`: its `Status` becomes the code.
            let out = out.expect("a Result");
            let inner = crate::class::args(&out)[0].clone();
            (
                syn::parse_quote!({
                    #take
                    #[allow(clippy::redundant_closure_call)]
                    (|| -> #out #body)().map_err(napi_support::coded)
                }),
                syn::parse_quote!(Result<#inner, String>),
            )
        } else {
            let t = out.unwrap_or_else(|| syn::parse_quote!(()));
            let mut ret = Returns(false);
            ret.visit_block_mut(&mut f.block.clone());
            let block = if ret.0 {
                // A closure, so the body's own `return` keeps its meaning.
                syn::parse_quote!({
                    #take
                    #[allow(clippy::redundant_closure_call)]
                    Ok((|| #body)())
                })
            } else if let [syn::Stmt::Expr(e, None)] = &body.stmts[..] {
                syn::parse_quote!({
                    #take
                    Ok(#e)
                })
            } else {
                syn::parse_quote!({
                    #take
                    Ok(#body)
                })
            };
            (block, syn::parse_quote!(Result<#t, String>))
        };
    f.block = block;
    f.sig.output = syn::parse_quote!(-> #ret);
}

/// Whether a body has a `return` of its own (not a closure's).
struct Returns(bool);

impl VisitMut for Returns {
    fn visit_expr_mut(&mut self, e: &mut syn::Expr) {
        match e {
            syn::Expr::Return(_) => self.0 = true,
            syn::Expr::Closure(_) => {}
            _ => syn::visit_mut::visit_expr_mut(self, e),
        }
    }
}

/// `self.<field>` becomes `(*__<field>)`, the value taken from the slot,
/// and `&self.<field>` in a `&self` method `__<field>` itself.
struct Reads<'a> {
    field: &'a Ident,
    local: &'a Ident,
    /// `__<field>` is a shared reference.
    shared: bool,
    found: bool,
}

impl Reads<'_> {
    fn is_field(&self, e: &syn::Expr) -> bool {
        matches!(e, syn::Expr::Field(f)
            if matches!(&f.member, syn::Member::Named(m) if m == self.field)
                && matches!(&*f.base, syn::Expr::Path(p) if p.path.is_ident("self")))
    }
}

impl VisitMut for Reads<'_> {
    fn visit_expr_mut(&mut self, e: &mut syn::Expr) {
        let local = self.local;
        if let syn::Expr::Reference(r) = e
            && r.mutability.is_none()
            && self.shared
            && self.is_field(&r.expr)
        {
            *e = syn::parse_quote!(#local);
            self.found = true;
            return;
        }
        if self.is_field(e) {
            *e = syn::parse_quote!((*#local));
            self.found = true;
            return;
        }
        syn::visit_mut::visit_expr_mut(self, e);
    }
}

/// A slotted class built anywhere (`Name { field: v }`, or `Self { field }`
/// in its own impl): the value goes in a new slot.
struct Built<'a> {
    targets: &'a BTreeMap<String, Target>,
    this: Option<&'a str>,
}

impl VisitMut for Built<'_> {
    fn visit_expr_struct_mut(&mut self, e: &mut syn::ExprStruct) {
        syn::visit_mut::visit_expr_struct_mut(self, e);
        let Some(last) = e.path.segments.last().map(|s| s.ident.to_string()) else {
            return;
        };
        let name = match last.as_str() {
            "Self" => self.this.map(str::to_string),
            _ => Some(last),
        };
        let Some(t) = name.as_deref().and_then(|n| self.targets.get(n)) else {
            return;
        };
        for fv in &mut e.fields {
            if matches!(&fv.member, syn::Member::Named(m) if *m == t.field) {
                let v = &fv.expr;
                fv.expr = syn::parse_quote!(napi_support::Slot::new(#v));
                fv.colon_token = Some(Default::default());
            }
        }
    }
}

/// Every class of `file` with a `#[napi]` `dispose()`: by namespace (`None`
/// for the top level), its JS names.
fn disposable(file: &syn::File) -> BTreeMap<Option<String>, BTreeSet<String>> {
    let mut out: BTreeMap<Option<String>, BTreeSet<String>> = BTreeMap::new();
    for (id, js, ns) in file.items.iter().filter_map(class_of) {
        let has = inherent_impls(std::slice::from_ref(file), &id.to_string())
            .flat_map(|imp| &imp.items)
            .any(|it| {
                matches!(it, ImplItem::Fn(f) if f.sig.ident == "dispose" && napi_args(&f.attrs).is_some())
            });
        if has {
            out.entry(ns).or_default().insert(js);
        }
    }
    out
}

/// What `using` needs for every class of `file` with a `dispose()`:
///
/// - a `#[napi(module_exports)]` hook setting `[Symbol.dispose]` to
///   `dispose` on each prototype when the module loads, only where the
///   runtime has the symbol. napi's `well_known_symbol` would fail the whole
///   module without it (Node before 20.4, iOS Safari as of 2026), so the
///   key is never declared to napi;
/// - the `.d.ts` header declaring it, by interface merging, each class
///   under its namespace. napi writes it at the top of the `.d.ts` (its
///   `dtsHeaderFile`), so it carries napi's own opening lines too.
pub fn aliases(file: &syn::File) -> (TokenStream, String) {
    let classes = disposable(file);
    let mut header = String::from(
        "/* auto-generated by NAPI-RS */\n/* eslint-disable */\n\n\
         // @generated by napi-gen: the classes with a `dispose()`. DO NOT EDIT.\n\
         // `[Symbol.dispose]` is set when the module loads, only where the\n\
         // runtime has the symbol.\n",
    );
    let mut paths = Vec::new();
    for (ns, names) in &classes {
        let (open, indent, close) = match ns {
            Some(ns) => (format!("export declare namespace {ns} {{\n"), "  ", "}\n"),
            None => (String::new(), "", ""),
        };
        header.push('\n');
        header.push_str(&open);
        for name in names {
            header.push_str(&format!(
                "{indent}export interface {name} {{\n{indent}  [Symbol.dispose](): void\n{indent}}}\n"
            ));
            let ns = match ns {
                Some(ns) => quote!(Some(#ns)),
                None => quote!(None),
            };
            paths.push(quote!((#ns, #name)));
        }
        header.push_str(close);
    }
    let code = quote! {
        #[doc = " `[Symbol.dispose]` on the prototype of every class with a `dispose()`,"]
        #[doc = " only where the runtime has the symbol: napi's `well_known_symbol` fails"]
        #[doc = " the whole module without it. The `.d.ts` header declares it."]
        #[napi(module_exports)]
        pub fn alias_dispose(env: Env, exports: Object) -> Result<()> {
            let symbol: Object = env.get_global()?.get_named_property_unchecked("Symbol")?;
            let key: Unknown = symbol.get_named_property("dispose")?;
            if key.get_type()? != ValueType::Symbol {
                return Ok(());
            }
            let classes: &[(Option<&str>, &str)] = &[#(#paths),*];
            for (ns, name) in classes {
                let scope: Object = match ns {
                    Some(ns) => exports.get_named_property_unchecked(ns)?,
                    None => exports,
                };
                let class: Object = scope.get_named_property_unchecked(name)?;
                let mut proto: Object = class.get_named_property_unchecked("prototype")?;
                let dispose: Unknown = proto.get_named_property("dispose")?;
                proto.set_property(key, dispose)?;
            }
            Ok(())
        }
    };
    (code, header)
}
