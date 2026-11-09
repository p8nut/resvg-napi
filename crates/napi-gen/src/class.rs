//! Classes and free functions of the namespaced surface, mirrored with what
//! makes them usable from JS on their own: constructors and statics, `Result`
//! errors, and the conversions Rust's traits and public fields offer (ADR
//! 0002, Q23, Q24, Q31, Q34).
//!
//! [`Classes`] is the table of every type `Config::classes` lists. Each one
//! holds its upstream value as its shape asks ([`Holding`]), and an argument
//! or a return naming a listed type, from any source, is that class. The
//! code is named as upstream names things (`Canvas`, `paint`): [`namespaced`]
//! adds the prefix and the namespace afterwards.
//!
//! Every generated function returns `Result<T, String>`: napi's `Result` with
//! a string code, so what it throws carries `code` and `message` (see
//! `napi_support::Thrown`).
//!
//! [`namespaced`]: crate::emit::namespaced

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{Fields, FnArg, ImplItem, Item, ReturnType, Type};

use crate::emit::{Site, js_conv, method_gate};
use crate::sources::{derives, inherent_impls, is_pub, self_ident};
use crate::vocab::*;
use crate::{config, namespace_of};

/// One source of classes and functions: its root path from the generated
/// code (`toy`, `toy::alt`), its files, and where its types are exported
/// (`ModTree::paths`).
pub struct Source<'a> {
    pub root: &'a str,
    pub files: &'a [syn::File],
    pub modules: &'a BTreeMap<String, String>,
}

/// How a class holds its upstream value, read off the type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Holding {
    /// `Copy`: held by value. A method taking `self` takes a copy, and
    /// `&mut self` writes the class's own value.
    Value,
    /// Held by value until a method taking `self` consumes it, after which
    /// every call throws: a type with such a method, or with `&mut self`
    /// methods and no `Clone`.
    Owned,
    /// Behind an `Arc`, read-only: a type with no `&mut self` method. Shared
    /// with whatever else holds the `Arc`, never copied.
    Shared,
    /// Behind an `Arc`, its `&mut self` methods through `Arc::make_mut`:
    /// a `Clone` type with `&mut self` methods and none taking `self`.
    SharedCow,
    /// Borrowed from the value it was reached from, which it keeps alive: a
    /// `napi_support::Projection` (`Config::projections`). Read-only.
    Projected,
}

/// One listed class.
pub struct Class {
    /// Index into [`Classes::sources`].
    pub source: usize,
    /// The upstream name, which is also the class's Rust ident and JS name.
    pub name: String,
    pub holding: Holding,
    /// The public fields, when every field is public and convertible: the
    /// class then gets `fromObject`, `toObject` and a getter and a setter per
    /// field. Empty otherwise.
    pub plain: Vec<(Ident, Prim)>,
    /// Another namespace has a class of the same name. napi keeps one
    /// constructor per JS class name and builds a returned instance from it,
    /// so such a class is only ever returned by its own constructor and
    /// factories, which build from the class they are called on.
    pub shared_name: bool,
    /// The union ([`Classes::unions`]) whose variant carries it, if any.
    pub of: Option<usize>,
}

/// The table, and what was emitted from it.
pub struct Classes<'a> {
    pub sources: Vec<Source<'a>>,
    pub classes: Vec<Class>,
    /// The enums `Config::projections` lists, each a union of projections.
    pub unions: Vec<crate::project::Union>,
    /// Error enums thrown by something emitted: (source, name). Each gets a
    /// type of its codes from [`Classes::error_codes`].
    errors: RefCell<BTreeSet<(usize, String)>>,
}

/// What a JS number, boolean or string becomes upstream, both ways.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prim {
    F32,
    F64,
    U8,
    U16,
    U32,
    I32,
    Usize,
    Bool,
    /// An owned `String`.
    Str,
}

impl Prim {
    pub(crate) fn of(ty: &Type) -> Option<Prim> {
        let segs = segs(ty)?;
        if segs.len() != 1 {
            return None;
        }
        Some(match segs[0].as_str() {
            "f32" => Prim::F32,
            "f64" => Prim::F64,
            "u8" => Prim::U8,
            "u16" => Prim::U16,
            "u32" => Prim::U32,
            "i32" => Prim::I32,
            "usize" => Prim::Usize,
            "bool" => Prim::Bool,
            "String" => Prim::Str,
            _ => return None,
        })
    }

    /// The type napi converts the JS value into.
    pub(crate) fn js(self) -> TokenStream {
        match self {
            Prim::F32 | Prim::F64 => quote!(f64),
            Prim::U8 | Prim::U16 | Prim::U32 | Prim::Usize => quote!(u32),
            Prim::I32 => quote!(i32),
            Prim::Bool => quote!(bool),
            Prim::Str => quote!(String),
        }
    }

    /// `e`, an upstream value, as JS. A `usize` saturates.
    fn js_out(self, e: &TokenStream) -> TokenStream {
        match self {
            Prim::F32 => quote!(f64::from(#e)),
            Prim::U8 | Prim::U16 => quote!(u32::from(#e)),
            Prim::Usize => quote!(u32::try_from(#e).unwrap_or(u32::MAX)),
            Prim::Str => quote!(#e.clone()),
            Prim::F64 | Prim::U32 | Prim::I32 | Prim::Bool => quote!(#e),
        }
    }

    /// `e`, a JS value, as upstream: an expression that may `?` on a number
    /// that does not fit. `name` is what the error calls it.
    pub(crate) fn js_in(self, e: &TokenStream, name: &str) -> TokenStream {
        let narrow = |t: TokenStream| quote!(#t::try_from(#e).map_err(|_| napi_support::invalid(#name, #e))?);
        match self {
            Prim::F32 => quote!(to_f32(#e)),
            Prim::U8 => narrow(quote!(u8)),
            Prim::U16 => narrow(quote!(u16)),
            Prim::Usize => narrow(quote!(usize)),
            Prim::F64 | Prim::U32 | Prim::I32 | Prim::Bool | Prim::Str => quote!(#e),
        }
    }
}

/// A type's path segments, references and lifetimes aside: `&a::B<'_>` is
/// `[a, B]`. None for anything but a path.
pub(crate) fn segs(ty: &Type) -> Option<Vec<String>> {
    match ty {
        Type::Path(p) if p.qself.is_none() => Some(
            p.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect(),
        ),
        Type::Group(g) => segs(&g.elem),
        Type::Paren(p) => segs(&p.elem),
        _ => None,
    }
}

/// The type arguments of a path's last segment: `Result<A, B>` is `[A, B]`.
pub(crate) fn args(ty: &Type) -> Vec<&Type> {
    let Type::Path(p) = ty else {
        return Vec::new();
    };
    let Some(syn::PathArguments::AngleBracketed(a)) = p.path.segments.last().map(|s| &s.arguments)
    else {
        return Vec::new();
    };
    a.args
        .iter()
        .filter_map(|a| match a {
            syn::GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// `&T` is `(Some(false), T)`, `&mut T` `(Some(true), T)`, `T` `(None, T)`.
pub(crate) fn deref(ty: &Type) -> (Option<bool>, &Type) {
    match ty {
        Type::Reference(r) => (Some(r.mutability.is_some()), &r.elem),
        _ => (None, ty),
    }
}

pub(crate) fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

/// The last segment is `name`, with `n` type arguments.
pub(crate) fn is(ty: &Type, name: &str, n: usize) -> bool {
    segs(ty).is_some_and(|s| s.last().is_some_and(|l| l == name)) && args(ty).len() == n
}

/// What a method's receiver is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Recv {
    None,
    Ref,
    Mut,
    Value,
}

pub(crate) fn receiver(sig: &syn::Signature) -> Option<Recv> {
    match sig.inputs.first() {
        Some(FnArg::Receiver(r)) => match &r.kind {
            syn::ReceiverKind::Reference(_, _, m) => {
                Some(if m.is_some() { Recv::Mut } else { Recv::Ref })
            }
            syn::ReceiverKind::Value => Some(Recv::Value),
            // `self: Box<Self>` and the like.
            _ => None,
        },
        _ => Some(Recv::None),
    }
}

pub(crate) fn find_struct<'f>(files: &'f [syn::File], name: &str) -> Option<&'f syn::ItemStruct> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Struct(s) if s.ident == name && is_pub(&s.vis) => Some(s),
        _ => None,
    })
}

/// A function's callback parameters, by name: `P` bounded by
/// `FnOnce(&[u8], u32) -> T` (inline or in the `where` clause) whose result
/// is another type parameter, with its inputs and that parameter's name.
fn callbacks(sig: &syn::Signature) -> BTreeMap<String, (Vec<Type>, String)> {
    let params: BTreeSet<String> = sig
        .generics
        .type_params()
        .map(|p| p.ident.to_string())
        .collect();
    let inline = sig
        .generics
        .type_params()
        .map(|p| (p.ident.to_string(), &p.bounds));
    let clause = sig
        .generics
        .where_clause
        .iter()
        .flat_map(|w| &w.predicates)
        .filter_map(|p| match p {
            syn::WherePredicate::Type(t) => Some((ty_str(&t.bounded_ty), &t.bounds)),
            _ => None,
        });
    let mut out = BTreeMap::new();
    for (name, bounds) in inline.chain(clause) {
        for b in bounds {
            let syn::TypeParamBound::Trait(t) = b else {
                continue;
            };
            let Some(last) = t.path.segments.last() else {
                continue;
            };
            let syn::PathArguments::Parenthesized(f) = &last.arguments else {
                continue;
            };
            if !["FnOnce", "FnMut", "Fn"].contains(&last.ident.to_string().as_str()) {
                continue;
            }
            if let ReturnType::Type(_, r) = &f.output
                && let Some([result]) = segs(r).as_deref()
                && params.contains(result)
            {
                out.insert(
                    name.clone(),
                    (
                        f.inputs.iter().map(|a| a.ty.clone()).collect(),
                        result.clone(),
                    ),
                );
            }
        }
    }
    out
}

/// The closure a callback gets, copying out each input it is handed (bytes,
/// a string, a number), and the type it returns: the value, or a tuple of
/// them. `None` when an input has no owned copy here.
fn copy_out(inputs: &[Type]) -> Option<(TokenStream, Type)> {
    let mut pats = Vec::new();
    let (mut vals, mut tys): (Vec<TokenStream>, Vec<TokenStream>) = (Vec::new(), Vec::new());
    for (i, ty) in inputs.iter().enumerate() {
        let a = format_ident!("__a{i}");
        let (r, inner) = deref(ty);
        let (val, owned) = match (r, inner) {
            (Some(false), Type::Slice(s)) if Prim::of(&s.elem) == Some(Prim::U8) => {
                (quote!(#a.to_vec()), quote!(Vec<u8>))
            }
            (Some(false), _) if matches!(segs(inner).as_deref(), Some([s]) if s == "str") => {
                (quote!(#a.to_string()), quote!(String))
            }
            (None, _) if Prim::of(inner).is_some() => (quote!(#a), quote!(#inner)),
            _ => return None,
        };
        pats.push(quote!(#a: #ty));
        vals.push(val);
        tys.push(owned);
    }
    let (body, ty) = match (vals.as_slice(), tys.as_slice()) {
        ([v], [t]) => (v.clone(), t.clone()),
        _ => (quote!((#(#vals),*)), quote!((#(#tys),*))),
    };
    Some((quote!(|#(#pats),*| #body), syn::parse2(ty).ok()?))
}

pub(crate) fn find_enum<'f>(files: &'f [syn::File], name: &str) -> Option<&'f syn::ItemEnum> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Enum(e) if e.ident == name && is_pub(&e.vis) => Some(e),
        _ => None,
    })
}

/// Whether `files` define a public type called `name`, of any kind.
fn defines(files: &[syn::File], name: &str) -> bool {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(crate::sources::item_ident)
        .any(|i| i == name)
}

/// `impl <Trait> for <ty>`: the impl, found by the trait's last segment.
fn trait_impl<'f>(files: &'f [syn::File], ty: &str, tr: &str) -> Option<&'f syn::ItemImpl> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Impl(imp)
            if imp
                .trait_
                .as_ref()
                .is_some_and(|(p, _)| p.segments.last().is_some_and(|s| s.ident == tr))
                && self_ident(imp).is_some_and(|id| id == ty) =>
        {
            Some(imp)
        }
        _ => None,
    })
}

fn holding(files: &[syn::File], name: &str) -> Holding {
    if derives(files, name, "Copy") {
        return Holding::Value;
    }
    let (mut consumes, mut mutates) = (false, false);
    for imp in inherent_impls(files, name) {
        for it in &imp.items {
            let ImplItem::Fn(f) = it else { continue };
            if !is_pub(&f.vis) {
                continue;
            }
            match receiver(&f.sig) {
                Some(Recv::Value) => consumes = true,
                Some(Recv::Mut) => mutates = true,
                _ => {}
            }
        }
    }
    match (consumes, mutates) {
        (true, _) => Holding::Owned,
        (false, true) if derives(files, name, "Clone") => Holding::SharedCow,
        (false, true) => Holding::Owned,
        (false, false) => Holding::Shared,
    }
}

/// The public fields, when every field is public and of a [`Prim`] type and
/// the struct can be built from them (not `#[non_exhaustive]`).
fn plain_fields(st: &syn::ItemStruct) -> Vec<(Ident, Prim)> {
    let Fields::Named(named) = &st.fields else {
        return Vec::new();
    };
    if st.attrs.iter().any(|a| a.path().is_ident("non_exhaustive")) {
        return Vec::new();
    }
    let fields: Option<Vec<_>> = named
        .named
        .iter()
        .map(|f| {
            (is_pub(&f.vis))
                .then(|| Prim::of(&f.ty).map(|p| (f.ident.clone().unwrap(), p)))
                .flatten()
        })
        .collect();
    fields.unwrap_or_default()
}

/// What one generated function hands napi: its parameters, the statements
/// before the call, and the call's arguments.
pub(crate) struct Args {
    pub(crate) params: Vec<TokenStream>,
    pub(crate) pre: Vec<TokenStream>,
    pub(crate) fwd: Vec<TokenStream>,
}

/// How a returned value becomes JS: its JS type, and the expression that
/// converts `__v`.
pub(crate) struct Ret {
    pub(crate) ty: TokenStream,
    pub(crate) conv: TokenStream,
}

/// Why a function or method was left out.
pub(crate) type Why = (Reason, Option<String>);

impl<'a> Classes<'a> {
    /// The table of `Config::classes`, over `sources`.
    pub fn new(sources: Vec<Source<'a>>) -> Self {
        let mut classes = Vec::new();
        for (root, name) in config().classes {
            let source = sources
                .iter()
                .position(|s| s.root == *root)
                .unwrap_or_else(|| panic!("Config::classes: no source with root {root}"));
            let files = sources[source].files;
            let st = find_struct(files, name)
                .unwrap_or_else(|| panic!("upstream: `pub struct {name}` not found in {root}"));
            assert!(
                st.generics.params.is_empty(),
                "{root}::{name}: a class cannot have generic parameters"
            );
            let holding = holding(files, name);
            let plain = match holding {
                Holding::Value | Holding::Owned => plain_fields(st),
                Holding::Shared | Holding::SharedCow | Holding::Projected => Vec::new(),
            };
            classes.push(Class {
                source,
                name: name.to_string(),
                holding,
                plain,
                shared_name: false,
                of: None,
            });
        }
        let unions = crate::project::projections(&sources, &mut classes);
        let ns = |c: &Class| namespace_of(sources[c.source].root);
        for i in 0..classes.len() {
            classes[i].shared_name = classes
                .iter()
                .any(|o| o.name == classes[i].name && ns(o) != ns(&classes[i]));
        }
        Classes {
            sources,
            classes,
            unions,
            errors: RefCell::new(BTreeSet::new()),
        }
    }

    /// The JS names more than one namespace gives a class: [`namespaced`]
    /// lets them be, because every instance of them is built by the class it
    /// is called on (see [`Class::shared_name`]).
    ///
    /// [`namespaced`]: crate::emit::namespaced
    pub fn shared_names(&self) -> BTreeSet<String> {
        self.classes
            .iter()
            .filter(|c| c.shared_name)
            .map(|c| c.name.clone())
            .collect()
    }

    /// The classes held by value (`Copy`), by upstream name, each with
    /// whether it is plain data and the root of its source: what
    /// `Vocab::classes` and `Vocab::plain` start from.
    pub fn by_value(&self) -> impl Iterator<Item = (&str, bool, &str)> {
        self.classes
            .iter()
            .filter(|c| c.holding == Holding::Value)
            .map(|c| {
                (
                    c.name.as_str(),
                    !c.plain.is_empty(),
                    self.sources[c.source].root,
                )
            })
    }

    /// The classes not held by value whose upstream type is `Clone`, by
    /// upstream name, each with the root of its source: what `Vocab::owned`
    /// starts from (`Js::Owned`).
    pub fn by_clone(&self) -> impl Iterator<Item = (&str, &str)> {
        self.classes
            .iter()
            .filter(|c| {
                c.holding != Holding::Value
                    && derives(self.sources[c.source].files, &c.name, "Clone")
            })
            .map(|c| (c.name.as_str(), self.sources[c.source].root))
    }

    /// Those of [`Classes::by_clone`] held as a projection: what
    /// `Vocab::projected` starts from.
    pub fn by_projection(&self) -> impl Iterator<Item = (&str, &str)> {
        self.by_clone().filter(|(name, root)| {
            self.classes.iter().any(|c| {
                c.holding == Holding::Projected
                    && c.name == *name
                    && self.sources[c.source].root == *root
            })
        })
    }

    /// Whether `ty` (an upstream root-relative key such as `a::Ty` or `Ty`)
    /// is a listed class of the source at `root`.
    pub fn lists(&self, root: &str, ty: &str) -> bool {
        self.classes
            .iter()
            .any(|c| self.sources[c.source].root == root && c.name == bare(ty))
    }

    /// The source a type named by `segs` in source `ctx` is in, among those
    /// `has` accepts: `ctx` itself for a bare name, the source whose root
    /// ends with the qualifier for a qualified one (`alt::Color` is in
    /// `toy::alt`), and otherwise the only one that has it.
    pub(crate) fn resolve(
        &self,
        ctx: usize,
        segs: &[String],
        has: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let (name, quals) = segs.split_last()?;
        let n = self.sources.len();
        let local = quals.is_empty() || matches!(quals, [q] if q == "crate" || q == "self");
        if local {
            if has(ctx) {
                return Some(ctx);
            }
            // `crate::Ty` from a module of the same crate.
            let krate = |s: usize| self.sources[s].root.split("::").next();
            let same: Vec<usize> = (0..n)
                .filter(|&s| krate(s) == krate(ctx) && has(s))
                .collect();
            if let [s] = same[..] {
                return Some(s);
            }
            // A bare name `ctx` defines itself is its own, listed or not.
            if defines(self.sources[ctx].files, name) {
                return None;
            }
        } else {
            let q = quals.join("::");
            if let Some(s) = (0..n).find(|&s| {
                let root = self.sources[s].root;
                (root == q || root.ends_with(&format!("::{q}"))) && has(s)
            }) {
                return Some(s);
            }
        }
        // A re-export, reached by another crate's name: the only one.
        let all: Vec<usize> = (0..n).filter(|&s| has(s)).collect();
        match all[..] {
            [s] => Some(s),
            _ => None,
        }
    }

    /// The class `ty` names from source `ctx`, `this` being `Self`.
    pub(crate) fn class_of(&self, ctx: usize, this: Option<usize>, ty: &Type) -> Option<usize> {
        let segs = segs(ty)?;
        if segs == ["Self"] {
            return this;
        }
        let name = segs.last()?;
        let s = self.resolve(ctx, &segs, |s| {
            self.classes
                .iter()
                .any(|c| c.source == s && c.name == *name)
        })?;
        let c = self
            .classes
            .iter()
            .position(|c| c.source == s && c.name == *name)?;
        // A re-export reaches a class by another crate's name, but another
        // crate's type of the same name is never a projection
        // (`other::Path` is not a projected `Path`).
        let quals = &segs[..segs.len() - 1];
        let foreign = !quals.is_empty()
            && !matches!(quals, [q] if q == "crate" || q == "self")
            && !self.sources[s].root.ends_with(&quals.join("::"));
        (!foreign || self.classes[c].holding != Holding::Projected).then_some(c)
    }

    pub(crate) fn ident(&self, c: usize) -> Ident {
        format_ident!("{}", self.classes[c].name)
    }

    /// The upstream path of a type of source `s`.
    pub(crate) fn path(&self, s: usize, name: &str) -> TokenStream {
        let src = &self.sources[s];
        let root: TokenStream = src.root.parse().expect("a source root that is a path");
        upstream_path(name, src.modules, &root)
    }

    /// Wraps an upstream value `v` of class `c`, by value.
    fn wrapped(&self, c: usize, v: &TokenStream) -> TokenStream {
        let id = self.ident(c);
        quote!(#id::__from(#v))
    }

    /// One argument, or why it has no JS form.
    fn arg(
        &self,
        ctx: usize,
        this: Option<usize>,
        pat: &Ident,
        ty: &Type,
        bounds: &BTreeMap<String, String>,
    ) -> Option<(TokenStream, Option<TokenStream>, TokenStream)> {
        let name = lower_camel(&pat.to_string());
        let (r, inner) = deref(ty);
        let segs = segs(inner);
        // A generic parameter, through its bound.
        if let Some([p]) = segs.as_deref()
            && let Some(b) = bounds.get(p)
        {
            let ok = r.is_none()
                && (b.contains("AsRef<std::path::Path>")
                    || b.contains("AsRef<Path>")
                    || b.contains("Into<String>"));
            return ok.then(|| (quote!(#pat: String), None, quote!(#pat)));
        }
        if let (Some(false), Type::Slice(s)) = (r, inner)
            && Prim::of(&s.elem) == Some(Prim::U8)
        {
            return Some((quote!(#pat: Uint8Array), None, quote!(&#pat)));
        }
        if r == Some(false) && matches!(segs.as_deref(), Some([s]) if s == "str") {
            return Some((quote!(#pat: String), None, quote!(&#pat)));
        }
        if r.is_none() && is(inner, "Vec", 1) && Prim::of(args(inner)[0]) == Some(Prim::U8) {
            return Some((quote!(#pat: Uint8Array), None, quote!(#pat.to_vec())));
        }
        if r.is_none()
            && let Some(p) = Prim::of(inner)
        {
            let js = p.js();
            let same = quote!(#pat);
            let conv = p.js_in(&same, &name);
            let pre = (conv.to_string() != same.to_string()).then(|| quote!(let #pat = #conv;));
            return Some((quote!(#pat: #js), pre, quote!(#pat)));
        }
        if let Some(c) = self.class_of(ctx, this, inner) {
            let id = self.ident(c);
            use Holding::*;
            let fwd = match (r, self.classes[c].holding) {
                (None, Value) => quote!(*#pat.__ref()?),
                (Some(false), Value | Owned | Shared | SharedCow) => quote!(#pat.__ref()?),
                (Some(false), Projected) => quote!(#pat.__ref()?.get()),
                (Some(true), Value | Owned) => quote!(#pat.__mut()?),
                (Some(true), SharedCow) => quote!(std::sync::Arc::make_mut(#pat.__mut()?)),
                _ => return None,
            };
            let param = if r == Some(true) {
                quote!(#pat: &mut #id)
            } else {
                quote!(#pat: &#id)
            };
            // Resolved before the call: `?` cannot run inside it.
            let tmp = format_ident!("__{pat}");
            return Some((param, Some(quote!(let #tmp = #fwd;)), quote!(#tmp)));
        }
        if r == Some(false)
            && let Some(u) = self.union_of(ctx, inner)
        {
            return Some(self.union_arg(u, pat));
        }
        let segs = segs?;
        let last = segs.last()?;
        // `Config::views`: borrowed from its class for the call.
        if r == Some(true)
            && let Some(v) = config().views.iter().find(|v| v.ty == last)
        {
            let mut class_segs = segs.clone();
            *class_segs.last_mut()? = v.class.to_string();
            let fake: Type = syn::parse_str(&class_segs.join("::")).ok()?;
            let c = self.class_of(ctx, this, &fake)?;
            let (id, via) = (self.ident(c), format_ident!("{}", v.via));
            let from = match self.classes[c].holding {
                Holding::Owned | Holding::Value => quote!(#pat.__mut()?),
                Holding::SharedCow => quote!(std::sync::Arc::make_mut(#pat.__mut()?)),
                Holding::Shared | Holding::Projected => return None,
            };
            let tmp = format_ident!("__{pat}");
            return Some((
                quote!(#pat: &mut #id),
                Some(quote!(let mut #tmp = #from.#via();)),
                quote!(&mut #tmp),
            ));
        }
        // An option object (`Config::option_objects`): optional, `Default`
        // under what JS provided.
        if r == Some(false) && config().option_objects.contains(&last.as_str()) {
            let s = self.resolve(ctx, &segs, |s| {
                crate::sources::has_default(self.sources[s].files, last)
            })?;
            let o = format_ident!("{last}");
            let tmp = format_ident!("__{pat}");
            // One borrowing lists JS passed converts them first, and keeps
            // both alive for the call.
            let pre = if crate::emit::has_input_lists(self.sources[s].files, last) {
                let (own, lists) = (format_ident!("__{pat}_o"), format_ident!("__{pat}_l"));
                quote! {
                    let #own = #pat.unwrap_or_default();
                    let #lists = #own.lists()?;
                    let #tmp = #own.to_upstream(&#lists)?;
                }
            } else {
                quote! {
                    let #tmp = #pat.unwrap_or_default().to_upstream()?;
                }
            };
            return Some((quote!(#pat: Option<#o>), Some(pre), quote!(&#tmp)));
        }
        // An input enum (`emit::is_enum_input`), converted for the call from
        // the `{ type, value }` JS passed: lent where upstream borrows it,
        // given where it takes it.
        if r != Some(true)
            && self
                .resolve(ctx, &segs, |s| {
                    find_enum(self.sources[s].files, last).is_some_and(crate::emit::is_enum_input)
                })
                .is_some()
        {
            let e = format_ident!("{last}");
            let tmp = format_ident!("__{pat}");
            return Some((
                quote!(#pat: #e),
                Some(quote!(let #tmp = #pat.to_upstream()?;)),
                if r.is_some() {
                    quote!(&#tmp)
                } else {
                    quote!(#tmp)
                },
            ));
        }
        None
    }

    /// The expression turning an error `e` of type `ty` into what is thrown:
    /// an upstream enum's variant name as `code`; `GenericFailure` for an
    /// error type no source defines.
    pub(crate) fn thrown(&self, ctx: usize, ty: Option<&Type>) -> TokenStream {
        let found = ty.and_then(|ty| {
            let segs = segs(ty)?;
            let name = segs.last()?.clone();
            let s = self.resolve(ctx, &segs, |s| {
                find_enum(self.sources[s].files, &name)
                    .is_some_and(|e| !e.attrs.iter().any(|a| a.path().is_ident("non_exhaustive")))
            })?;
            Some((s, name))
        });
        let Some((s, name)) = found else {
            return quote!(|e| napi_support::thrown("GenericFailure", e));
        };
        let e = find_enum(self.sources[s].files, &name).expect("resolved above");
        let path = self.path(s, &name);
        let arms = e.variants.iter().map(|v| {
            let (id, code) = (&v.ident, v.ident.to_string());
            let rest = match &v.fields {
                Fields::Unit => quote!(),
                Fields::Unnamed(_) => quote!((..)),
                Fields::Named(_) => quote!({ .. }),
            };
            quote!(#path::#id #rest => #code,)
        });
        self.errors.borrow_mut().insert((s, name));
        quote!(|e| {
            let code = match &e { #(#arms)* };
            napi_support::thrown(code, &e)
        })
    }

    /// How a returned upstream `ty` becomes JS, or why it cannot. `own` is
    /// whether this is the class's own constructor or factory, the only
    /// place a class with a shared name may be returned; `with_self` whether
    /// the expression may use `self` (a method, not a static).
    pub(crate) fn ret(
        &self,
        ctx: usize,
        this: Option<usize>,
        ty: &Type,
        own: bool,
        with_self: bool,
        vocab: &Vocab,
    ) -> Result<Ret, Reason> {
        let v = quote!(__v);
        if is_unit(ty) {
            return Ok(Ret {
                ty: quote!(()),
                conv: v,
            });
        }
        let (r, inner) = deref(ty);
        if let Type::Slice(s) = inner
            && r == Some(false)
            && Prim::of(&s.elem) == Some(Prim::U8)
        {
            return Ok(Ret {
                ty: quote!(Buffer),
                conv: quote!(#v.to_vec().into()),
            });
        }
        if r.is_none() && is(inner, "Vec", 1) && Prim::of(args(inner)[0]) == Some(Prim::U8) {
            return Ok(Ret {
                ty: quote!(Buffer),
                conv: quote!(#v.into()),
            });
        }
        if r == Some(false) && matches!(segs(inner).as_deref(), Some([s]) if s == "str") {
            return Ok(Ret {
                ty: quote!(String),
                conv: quote!(#v.to_string()),
            });
        }
        if r.is_none()
            && let Some(p) = Prim::of(inner)
        {
            let conv = match p {
                Prim::Str => v,
                _ => p.js_out(&v),
            };
            return Ok(Ret { ty: p.js(), conv });
        }
        // A tuple of numbers, booleans, strings or bytes: an array, `[A, B]`.
        if let (None, Type::Tuple(t)) = (r, inner)
            && t.elems.len() > 1
            && let Some(elems) = t
                .elems
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let i = syn::Index::from(i);
                    let at = quote!(#v.#i);
                    if is(e, "Vec", 1) && Prim::of(args(e)[0]) == Some(Prim::U8) {
                        return Some((quote!(Buffer), quote!(#at.into())));
                    }
                    if let Some(p) = Prim::of(e) {
                        return Some((p.js(), p.js_out(&at)));
                    }
                    // Anything else the vocabulary maps for a getter: a
                    // union, a class held by value.
                    let t = ty_str(e);
                    let js = vocab.classify(&t)?;
                    js_conv(&js, &t, vocab, &at, Site::Getter)
                        .filter(|c| !c.is_result)
                        .map(|c| (c.ty, c.expr))
                })
                .collect::<Option<Vec<_>>>()
        {
            let (tys, convs): (Vec<_>, Vec<_>) = elems.into_iter().unzip();
            return Ok(Ret {
                ty: quote!((#(#tys),*)),
                conv: quote!((#(#convs),*)),
            });
        }
        // `Arc<T>` of a class holding an `Arc`.
        if is(inner, "Arc", 1)
            && let Some(c) = self.class_of(ctx, this, args(inner)[0])
            && matches!(
                self.classes[c].holding,
                Holding::Shared | Holding::SharedCow
            )
        {
            if self.classes[c].shared_name && !own {
                return Err(Reason::SharedName);
            }
            let id = self.ident(c);
            let arc = match r {
                Some(_) => quote!(std::sync::Arc::clone(#v)),
                None => v,
            };
            return Ok(Ret {
                ty: quote!(#id),
                conv: quote!(#id::wrap(#arc)),
            });
        }
        // Several values of a class held by value: a `Vec<T>`, or an inline
        // vector over an array (`X<[T; N]>`). Either iterates by value.
        if r.is_none()
            && let [elem] = args(inner)[..]
            && let Some(item) = match elem {
                Type::Array(a) => Some(&*a.elem),
                _ if is(inner, "Vec", 1) => Some(elem),
                _ => None,
            }
            && let Some(c) = self.class_of(ctx, this, item)
            && self.classes[c].holding == Holding::Value
        {
            if self.classes[c].shared_name && !own {
                return Err(Reason::SharedName);
            }
            let id = self.ident(c);
            let each = self.wrapped(c, &quote!(x));
            return Ok(Ret {
                ty: quote!(Vec<#id>),
                conv: quote!(#v.into_iter().map(|x| #each).collect()),
            });
        }
        if let Some(c) = self.class_of(ctx, this, inner) {
            if self.classes[c].shared_name && !own {
                return Err(Reason::SharedName);
            }
            let id = self.ident(c);
            let conv = match (r, self.classes[c].holding) {
                (None, _) => self.wrapped(c, &v),
                (Some(false), Holding::Value) => self.wrapped(c, &quote!(*#v)),
                _ => return Err(Reason::Returns),
            };
            return Ok(Ret {
                ty: quote!(#id),
                conv,
            });
        }
        // What the vocabulary maps, for a method: a handle class hands out
        // the handles and data it reaches.
        if with_self {
            let t = ty_str(ty);
            // A getter's conversion where a method has none (a payload
            // enum, a list of numbers): the call is the same.
            if let Some(js) = vocab.classify(&t)
                && let Some(c) = js_conv(&js, &t, vocab, &v, Site::Method)
                    .or_else(|| js_conv(&js, &t, vocab, &v, Site::Getter))
                && !c.is_result
            {
                return Ok(Ret {
                    ty: c.ty,
                    conv: c.expr,
                });
            }
        }
        Err(Reason::Returns)
    }

    /// One function: a method of class `this` (`recv` set), a static of it,
    /// or a free function (`this` None). `call` is what the arguments are
    /// applied to. Returns the `#[napi]` item, or why it was left out.
    #[allow(clippy::too_many_arguments)]
    fn function(
        &self,
        ctx: usize,
        this: Option<usize>,
        sig: &syn::Signature,
        attrs: &[syn::Attribute],
        gate: Option<(TokenStream, TokenStream)>,
        napi: TokenStream,
        vocab: &Vocab,
    ) -> Result<TokenStream, Why> {
        let name = sig.ident.to_string();
        let skip = |r: Reason, t: Option<String>| Err((r, t));
        if matches!(sig.safety, syn::Safety::Unsafe(_)) {
            return skip(Reason::Unsafe, None);
        }
        let recv = receiver(sig).ok_or((Reason::NoReceiver, None))?;
        let holding = this.map(|c| self.classes[c].holding);
        let ident = &sig.ident;
        let is_new = this.is_some() && recv == Recv::None && name == "new";
        // The receiver, resolved before the call.
        let me = match (recv, holding) {
            (Recv::None, _) => None,
            (Recv::Ref, Some(Holding::Projected)) => {
                Some((quote!(&self), quote!(let __self = self.__ref()?.get();)))
            }
            (Recv::Ref, Some(_)) => Some((quote!(&self), quote!(let __self = self.__ref()?;))),
            (Recv::Mut, Some(Holding::Value | Holding::Owned)) => {
                Some((quote!(&mut self), quote!(let __self = self.__mut()?;)))
            }
            (Recv::Mut, Some(Holding::SharedCow)) => Some((
                quote!(&mut self),
                quote!(let __self = std::sync::Arc::make_mut(self.__mut()?);),
            )),
            (Recv::Mut, _) => return skip(Reason::NeedsMut, None),
            // A copy: the class keeps its own.
            (Recv::Value, Some(Holding::Value)) => {
                Some((quote!(&self), quote!(let __self = *self.__ref()?;)))
            }
            // Taken out: the class is spent.
            (Recv::Value, Some(Holding::Owned)) => {
                Some((quote!(&mut self), quote!(let __self = self.__take()?;)))
            }
            (Recv::Value, _) => return skip(Reason::ConsumesSelf, None),
            (_, None) => return skip(Reason::NoReceiver, None),
        };

        let bounds: BTreeMap<String, String> = sig
            .generics
            .type_params()
            .map(|p| (p.ident.to_string(), quote!(#p).to_string().replace(' ', "")))
            .collect();
        let mut a = Args {
            params: Vec::new(),
            pre: Vec::new(),
            fwd: Vec::new(),
        };
        let callbacks = callbacks(sig);
        // The type a callback's result stands for: what its closure returns.
        let mut copied: Option<(String, Type)> = None;
        for input in sig.inputs.iter().skip(usize::from(recv != Recv::None)) {
            let FnArg::Typed(t) = input else { continue };
            let syn::Pat::Ident(p) = &*t.pat else {
                return skip(Reason::Arg, Some(ty_str(&t.ty)));
            };
            // A callback whose result the function returns (`with_data(id,
            // |data, index| ..)`): no JS argument; the closure copies out what
            // it is handed, and that is what JS gets back.
            if let Some([seg]) = segs(&t.ty).as_deref()
                && let Some((inputs, result)) = callbacks.get(seg)
            {
                let Some((closure, ty)) = copy_out(inputs) else {
                    return skip(Reason::Arg, Some(ty_str(&t.ty)));
                };
                a.fwd.push(closure);
                copied = Some((result.clone(), ty));
                continue;
            }
            let Some((param, pre, fwd)) = self.arg(ctx, this, &p.ident, &t.ty, &bounds) else {
                return skip(Reason::Arg, Some(ty_str(&t.ty)));
            };
            a.params.push(param);
            a.pre.extend(pre);
            a.fwd.push(fwd);
        }

        // Result<T, E>, then Option<T>, then T.
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
        let out = match &copied {
            Some((result, ty)) if matches!(segs(&out).as_deref(), Some([s]) if s == result) => {
                ty.clone()
            }
            Some(_) => return skip(Reason::Returns, Some(ty_str(&out))),
            None => out,
        };
        // Only a class's own constructor and factories build it from the
        // class they are called on.
        let target = if is(&out, "Arc", 1) {
            args(&out)[0]
        } else {
            deref(&out).1
        };
        let returns_this = this.is_some() && self.class_of(ctx, this, target) == this;
        let factory = recv == Recv::None && returns_this && !optional;
        let label = match this {
            Some(c) => format!("{}::{name}", self.classes[c].name),
            None => name.clone(),
        };
        let self_pre = me.as_ref().map(|(_, p)| p);
        // A borrow out of a projection or an `Arc`: a projection again.
        let projected = match (&err, holding) {
            (None, Some(h)) if me.is_some() => {
                self.projected(h, ctx, ident, &out, optional, &a, vocab)
            }
            _ => None,
        };
        let (body, ret_ty) = match projected {
            Some(p) => {
                let (ty, bind, value) = p.map_err(|r| (r, Some(ty_str(&out))))?;
                let pre = &a.pre;
                let body = quote! {
                    #(#pre)*
                    #self_pre
                    #bind
                    napi_support::contained(#label, || #value).map_err(napi_support::coded)
                };
                (body, ty)
            }
            None => {
                // `Option<()>` says whether it did anything: `true` or `null`.
                let ret = if is_unit(&out) && optional {
                    Ret {
                        ty: quote!(bool),
                        conv: quote!(true),
                    }
                } else {
                    self.ret(ctx, this, &out, factory || is_new, me.is_some(), vocab)
                        .map_err(|r| (r, Some(ty_str(&out))))?
                };
                if is_new && !returns_this {
                    return skip(Reason::Returns, Some(ty_str(&out)));
                }

                let path = match this {
                    Some(c) => {
                        let p = self.path(self.classes[c].source, &self.classes[c].name);
                        quote!(<#p>)
                    }
                    None => {
                        let root: TokenStream =
                            self.sources[ctx].root.parse().expect("a root path");
                        root
                    }
                };
                let call = match me {
                    Some(_) => quote!(__self.#ident),
                    None => quote!(#path::#ident),
                };
                let fwd = &a.fwd;
                let pre = &a.pre;
                // A function of no argument is the closure itself.
                let run = if me.is_none() && fwd.is_empty() {
                    call
                } else {
                    quote!(|| #call(#(#fwd),*))
                };
                let mut value =
                    quote!(napi_support::contained(#label, #run).map_err(napi_support::coded)?);
                if let Some(e) = &err {
                    let thrown = self.thrown(ctx, e.as_ref());
                    value = quote!(#value.map_err(#thrown)?);
                }
                // The receiver last: a method taking `self` takes the value only
                // once every argument converted.
                let mut body = quote! {
                    #(#pre)*
                    #self_pre
                };
                let (js_ty, value) = if is_unit(&out) && !optional {
                    body.extend(quote!(#value;));
                    (&ret.ty, &quote!(()))
                } else {
                    body.extend(quote!(let __v = #value;));
                    (&ret.ty, &ret.conv)
                };
                let ret_ty = if is_new {
                    if optional {
                        let why = format!("{label} returned None: no value for these arguments");
                        body.extend(quote! {
                            let __v = __v.ok_or_else(|| napi_support::thrown("InvalidArg", #why))?;
                        });
                    }
                    body.extend(quote!(Ok(#value)));
                    quote!(Self)
                } else if optional {
                    body.extend(quote!(Ok(__v.map(|__v| #value))));
                    quote!(Option<#js_ty>)
                } else {
                    body.extend(quote!(Ok(#value)));
                    if factory {
                        quote!(Self)
                    } else {
                        quote!(#js_ty)
                    }
                };
                (body, ret_ty)
            }
        };
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
                        { Err(napi_support::thrown("GenericFailure", #missing)) }
                    },
                )
            }
        };
        let napi = if is_new {
            quote!(#[napi(constructor)])
        } else if factory && self.classes[this.expect("a factory's class")].shared_name {
            quote!(#[napi(factory)])
        } else {
            napi
        };
        let recv = me.map(|(r, _)| r);
        let params = recv.into_iter().chain(a.params);
        let doc = docs(attrs);
        Ok(quote! {
            #(#doc)*
            #napi
            #cfg_attr
            // `.map(|v| Class::__from(v))` is the one conversion table at
            // work, whatever the conversion.
            #[allow(clippy::redundant_closure)]
            pub fn #ident(#(#params),*) -> Result<#ret_ty, String> {
                #body
            }
        })
    }

    /// The class `c`: its struct, its helpers, and a `#[napi] impl` of
    /// everything that maps, with what did not.
    pub fn class(&self, c: usize, vocab: &Vocab) -> (TokenStream, Vec<Skip>) {
        let class = &self.classes[c];
        let src = &self.sources[class.source];
        let files = src.files;
        let name = &class.name;
        let id = self.ident(c);
        let up = self.path(class.source, name);
        let st = find_struct(files, name).expect("checked by Classes::new");
        let mut skipped = Vec::new();
        let mut methods = TokenStream::new();
        // Rust idents taken in the `#[napi] impl`.
        let mut taken: BTreeSet<String> = BTreeSet::new();
        // The members given an async twin (`Config::asynchronous`).
        let mut twins: Vec<String> = Vec::new();

        for imp in inherent_impls(files, name) {
            for it in &imp.items {
                let ImplItem::Fn(f) = it else { continue };
                if !is_pub(&f.vis) || !taken.insert(f.sig.ident.to_string()) {
                    continue;
                }
                let gate = method_gate(imp, f);
                let member = format!("{name}::{}", f.sig.ident);
                match self.function(
                    class.source,
                    Some(c),
                    &f.sig,
                    &f.attrs,
                    gate.clone(),
                    quote!(#[napi]),
                    vocab,
                ) {
                    Ok(code) => {
                        methods.extend(code);
                        if crate::twin::listed(src.root, &member) {
                            let twin = format!("{}_async", f.sig.ident);
                            assert!(
                                taken.insert(twin),
                                "{member}: its async twin's name is taken"
                            );
                            methods.extend(self.twin(
                                class.source,
                                Some(c),
                                &f.sig,
                                gate,
                                None,
                                vocab,
                            ));
                            twins.push(member);
                        }
                    }
                    Err((reason, ty)) => {
                        taken.remove(&f.sig.ident.to_string());
                        skipped.push(Skip::new(&f.sig.ident.to_string(), ty.as_deref(), reason));
                    }
                }
            }
        }
        for (r, member) in config().asynchronous {
            assert!(
                *r != src.root
                    || member.split_once("::").is_none_or(|(t, _)| t != name)
                    || twins.iter().any(|m| m == member),
                "Config::asynchronous lists {r}::{member}, which has no JS form to make async"
            );
        }

        // Conversions a Rust trait offers, one direction each (Q34).
        let factory = if class.shared_name {
            quote!(#[napi(factory)])
        } else {
            quote!(#[napi])
        };
        // The value: a reference to it, and the place itself.
        let (read, place_mut) = match class.holding {
            Holding::Projected => (quote!(self.__ref()?.get()), quote!(self.__ref()?.get())),
            _ => (quote!(self.__ref()?), quote!(self.__mut()?)),
        };
        if let Some(imp) = trait_impl(files, name, "FromStr")
            && taken.insert("from_str".into())
        {
            let err = imp.items.iter().find_map(|i| match i {
                ImplItem::Type(t) if t.ident == "Err" => Some(&t.ty),
                _ => None,
            });
            let thrown = self.thrown(class.source, err);
            let doc = format!(
                " Parses a `{name}`, as Rust's `FromStr` does. Throws when it does not parse."
            );
            methods.extend(quote! {
                #[doc = #doc]
                #factory
                #[allow(clippy::should_implement_trait)]
                pub fn from_str(s: String) -> Result<Self, String> {
                    let __v = s.parse::<#up>().map_err(#thrown)?;
                    Ok(Self::__from(__v))
                }
            });
        }
        if trait_impl(files, name, "Display").is_some() && taken.insert("to_string".into()) {
            let doc = format!(" The `{name}` as Rust's `Display` writes it.");
            methods.extend(quote! {
                #[doc = #doc]
                #[napi]
                pub fn to_string(&self) -> Result<String, String> {
                    Ok(#read.to_string())
                }
            });
        }

        // Plain data: a field getter and setter each, and the pair of
        // conversions to and from a plain object, which always come together.
        let mut extra = TokenStream::new();
        if !class.plain.is_empty() {
            let obj = format_ident!("{name}Object");
            let fields: Vec<&Ident> = class.plain.iter().map(|(f, _)| f).collect();
            let js: Vec<TokenStream> = class.plain.iter().map(|(_, p)| p.js()).collect();
            let mut to_js = Vec::new();
            let mut from_js = Vec::new();
            for (f, p) in &class.plain {
                let label = lower_camel(&f.to_string());
                let (get, set) = (format_ident!("field_{f}"), format_ident!("set_field_{f}"));
                let (jt, read_f) = (p.js(), p.js_out(&quote!(__v.#f)));
                let write_f = match p.js_in(&quote!(value), &label) {
                    same if same.to_string() == "value" => None,
                    conv => Some(quote!(let value = #conv;)),
                };
                to_js.push(read_f.clone());
                from_js.push(p.js_in(&quote!(obj.#f), &label));
                methods.extend(quote! {
                    #[napi(getter, js_name = #label)]
                    pub fn #get(&self) -> Result<#jt, String> {
                        let __v = #read;
                        Ok(#read_f)
                    }
                    #[napi(setter, js_name = #label)]
                    pub fn #set(&mut self, value: #jt) -> Result<(), String> {
                        #write_f
                        #place_mut.#f = value;
                        Ok(())
                    }
                });
            }
            let obj_doc = format!(" A `{name}`'s public fields, as a plain object.");
            extra.extend(quote! {
                #[doc = #obj_doc]
                #[napi(object)]
                #[derive(Clone)]
                pub struct #obj {
                    #(pub #fields: #js,)*
                }

                // Where a class cannot go (an object field, a union's
                // payload), the value is this object (`Js::Class`).
                impl From<&#up> for #obj {
                    fn from(__v: &#up) -> Self {
                        Self { #(#fields: #to_js,)* }
                    }
                }
            });
            if taken.insert("from_object".into()) && taken.insert("to_object".into()) {
                let doc = format!(" A `{name}` from a plain object of its fields.");
                methods.extend(quote! {
                    #[doc = #doc]
                    #factory
                    pub fn from_object(obj: #obj) -> Result<Self, String> {
                        Ok(Self::__from(#up { #(#fields: #from_js,)* }))
                    }
                    #[doc = " Its fields, as a plain object `fromObject` takes back."]
                    #[napi]
                    pub fn to_object(&self) -> Result<#obj, String> {
                        let __v = #read;
                        Ok(#obj { #(#fields: #to_js,)* })
                    }
                });
            }
        }

        let root = src.root;
        let mut doc = docs(&st.attrs);
        let note = match class.holding {
            Holding::Value => format!(" A `{root}::{name}`, held by value."),
            Holding::Owned => format!(
                " A `{root}::{name}`, owned. A method that consumes it in Rust leaves this\n instance spent: every later call throws."
            ),
            Holding::Shared => format!(" A shared, read-only `{root}::{name}`."),
            Holding::SharedCow => {
                format!(" A shared `{root}::{name}`, copied on the first write while shared.")
            }
            Holding::Projected => format!(
                " A `{root}::{name}` of its own, or borrowed from what it was reached from,\n which it keeps alive. Each call finds it there again."
            ),
        };
        if !doc.is_empty() {
            doc.push(quote!(#[doc = ""]));
        }
        for line in note.lines() {
            doc.push(quote!(#[doc = #line]));
        }
        // Every holding sits in a `napi_support::Slot`: `dispose()` empties
        // it, as a method taking `self` does, and every call after throws.
        let label = name.as_str();
        let (held, def, from) = match class.holding {
            Holding::Value | Holding::Owned => (
                up.clone(),
                quote!(pub struct #id { inner: napi_support::Slot<#up> }),
                quote! {
                    fn __from(inner: #up) -> Self { Self { inner: napi_support::Slot::new(inner) } }
                },
            ),
            Holding::Shared | Holding::SharedCow => (
                quote!(std::sync::Arc<#up>),
                quote!(pub struct #id { inner: napi_support::Slot<std::sync::Arc<#up>> }),
                quote! {
                    fn wrap(inner: std::sync::Arc<#up>) -> Self { Self { inner: napi_support::Slot::new(inner) } }
                    fn __from(inner: #up) -> Self { Self::wrap(std::sync::Arc::new(inner)) }
                },
            ),
            Holding::Projected => {
                let (def, from) = self.projected_class(c, &up);
                (quote!(napi_support::Projection<#up>), def, from)
            }
        };
        let mut helpers = quote! {
            #from
            fn __ref(&self) -> Result<&#held, String> { self.inner.get(#label) }
            fn __mut(&mut self) -> Result<&mut #held, String> { self.inner.get_mut(#label) }
        };
        if class.holding == Holding::Owned {
            helpers.extend(quote! {
                fn __take(&mut self) -> Result<#up, String> { self.inner.take(#label) }
            });
        }
        // Lent to an async twin (`crate::twin`): what it cannot clone.
        if matches!(
            class.holding,
            Holding::Value | Holding::Owned | Holding::SharedCow
        ) {
            helpers.extend(quote! {
                fn __lease(&mut self) -> Result<napi_support::Lease<#held>, String> {
                    self.inner.lease(#label)
                }
            });
        }
        // A union's value also holds the projection it was reached through.
        let of = class.of.is_some().then(|| quote!(self.of = None;));
        for m in ["dispose", "disposed"] {
            assert!(
                taken.insert(m.to_string()),
                "{name}: `{m}` is the generated dispose()"
            );
        }
        let what = match class.holding {
            Holding::Value => {
                " Ends this instance: every later call throws `Disposed`. It holds a copy,\n so nothing is freed: this is for uniformity."
            }
            Holding::Owned => {
                " Frees the value now, not at garbage collection: every later call throws\n `Disposed`."
            }
            Holding::Shared | Holding::SharedCow | Holding::Projected => {
                " Drops this instance's reference now, not at garbage collection: every\n later call throws `Disposed`. The value lives on while another instance\n holds it."
            }
        };
        let using = " Also `[Symbol.dispose]` where the runtime has the symbol, for `using`.\n Harmless twice.";
        // Only a value an async twin can lease can be in use.
        let leased = matches!(
            class.holding,
            Holding::Value | Holding::Owned | Holding::SharedCow
        )
        .then_some(" Throws `Leased` while an async call is using it.");
        let dispose_doc = what
            .lines()
            .chain(leased)
            .chain(using.lines())
            .map(|l| quote!(#[doc = #l]));
        methods.extend(quote! {
            #(#dispose_doc)*
            #[napi]
            pub fn dispose(&mut self) -> Result<(), String> {
                self.inner.dispose(#label)?;
                #of
                Ok(())
            }
            #[doc = " Whether `dispose()` has been called."]
            #[napi(getter)]
            pub fn disposed(&self) -> bool {
                self.inner.disposed()
            }
        });
        let code = quote! {
            #(#doc)*
            #[napi]
            #def

            impl #id {
                #helpers
            }

            #[napi]
            impl #id {
                #methods
            }

            #extra
        };
        (code, skipped)
    }

    /// The free functions `Config::functions` lists for the source at
    /// `root`, found among `files` (the crate root's), with what did not map.
    pub fn functions(
        &self,
        root: &str,
        files: &[syn::File],
        vocab: &Vocab,
    ) -> (TokenStream, Vec<Skip>) {
        let ctx = self
            .sources
            .iter()
            .position(|s| s.root == root)
            .unwrap_or_else(|| panic!("Config::functions: no source with root {root}"));
        let ns =
            namespace_of(root).unwrap_or_else(|| panic!("no Config::namespaces entry for {root}"));
        let mut code = TokenStream::new();
        let mut skipped = Vec::new();
        for (r, name) in config().functions.iter().filter(|(r, _)| *r == root) {
            let f = files
                .iter()
                .flat_map(|f| &f.items)
                .find_map(|i| match i {
                    Item::Fn(f) if f.sig.ident == name && is_pub(&f.vis) => Some(f),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("upstream: `pub fn {name}` not found in {r}"));
            let gate = match crate::cfg::gate(&crate::cfg::of(&f.attrs)) {
                crate::cfg::Gate::Always => None,
                crate::cfg::Gate::When(p, n) => Some((p, n)),
            };
            let js = lower_camel(name);
            let napi = quote!(#[napi(js_name = #js, namespace = #ns)]);
            let twin = crate::twin::listed(root, name);
            match self.function(ctx, None, &f.sig, &f.attrs, gate.clone(), napi, vocab) {
                Ok(c) => code.extend(c),
                Err((reason, ty)) => {
                    assert!(
                        !twin,
                        "Config::asynchronous lists {root}::{name}, which has no JS form to make async"
                    );
                    skipped.push(Skip::new(name, ty.as_deref(), reason));
                }
            }
            if twin {
                let js = format!("{js}Async");
                code.extend(self.twin(ctx, None, &f.sig, gate, Some((&js, ns)), vocab));
            }
        }
        (code, skipped)
    }

    /// The type of codes each thrown error enum gets (Q23), by source:
    /// `<Enum>Code`, the variant names as a string enum.
    pub fn error_codes(&self) -> Vec<(usize, TokenStream)> {
        self.errors
            .borrow()
            .iter()
            .map(|(s, name)| {
                let e = find_enum(self.sources[*s].files, name).expect("an error enum found before");
                let id = format_ident!("{name}Code");
                let variants = e.variants.iter().map(|v| &v.ident);
                let doc = format!(
                    " The `code` a thrown `{}::{name}` carries: the variant's name. The message is its `Display`.",
                    self.sources[*s].root
                );
                (
                    *s,
                    quote! {
                        #[doc = #doc]
                        #[napi(string_enum)]
                        pub enum #id { #(#variants,)* }
                    },
                )
            })
            .collect()
    }
}
