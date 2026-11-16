//! What a type is built of, read once from `syn`.
//!
//! Every phase asks the same few questions of a type: is it a reference, an
//! `Option`, a list, an `Arc`? What does it name? [`Shape::of`] answers them
//! for the outermost constructor, its parts borrowed from the `syn::Type`, so
//! a caller looks one level further with another `Shape::of` and still holds
//! the `syn` node to emit. One reading of a type, rather than a string
//! peeled by prefix in one place and a `syn` match in the next.

use quote::ToTokens;
use syn::{GenericArgument, PathArguments, Type, TypeParamBound};

/// The outermost constructor of a type.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape<'a> {
    /// `&T`, `&mut T`, `&'a T`. `lifetime`: one is named.
    Ref {
        mutable: bool,
        lifetime: bool,
        inner: &'a Type,
    },
    Option(&'a Type),
    Vec(&'a Type),
    Box(&'a Type),
    /// `Arc<T>`, by any path to it: `std::sync::Arc<T>`, `alloc::sync::Arc<T>`.
    Arc(&'a Type),
    /// `Result<T, E>`, or `Result<T>` (a crate's own alias).
    Result {
        ok: &'a Type,
        err: Option<&'a Type>,
    },
    /// `[T]`.
    Slice(&'a Type),
    /// `[T; N]`.
    Array(&'a Type),
    /// `impl Iterator<Item = T> + ..`.
    ImplIter(&'a Type),
    /// `(A, B)`; `()` is the empty one.
    Tuple(Vec<&'a Type>),
    /// Any other path: its segments as written, and the arguments of the
    /// last one (types, lifetimes, bindings alike).
    Path {
        segs: Vec<String>,
        args: Vec<&'a GenericArgument>,
    },
    /// `dyn AsRef<[u8]>`, with nothing beside it but `Send`, `Sync` and
    /// `'static`, in any order: bytes a crate shares without saying what
    /// holds them (a `Vec`, a file mapping).
    DynBytes,
    /// Anything else: a qualified-self path, a pointer, a function...
    Other,
}

/// The well-known wrappers are told by their last segment, so any path to
/// them reads the same.
fn wrapper<'a>(name: &str, tys: &[&'a Type]) -> Option<Shape<'a>> {
    Some(match (name, tys) {
        ("Option", [t]) => Shape::Option(t),
        ("Vec", [t]) => Shape::Vec(t),
        ("Box", [t]) => Shape::Box(t),
        ("Arc", [t]) => Shape::Arc(t),
        ("Result", [ok]) => Shape::Result { ok, err: None },
        ("Result", [ok, err]) => Shape::Result { ok, err: Some(err) },
        _ => return None,
    })
}

/// Whether a bound is the trait path `name`, written bare, and its arguments.
fn bound_is<'a>(b: &'a TypeParamBound, name: &str) -> Option<&'a PathArguments> {
    match b {
        TypeParamBound::Trait(t)
            if t.path.segments.len() == 1 && t.path.segments[0].ident == name =>
        {
            Some(&t.path.segments[0].arguments)
        }
        _ => None,
    }
}

/// The one type argument of `<..>`.
fn one_type(a: &PathArguments) -> Option<&Type> {
    match a {
        PathArguments::AngleBracketed(a) if a.args.len() == 1 => match &a.args[0] {
            GenericArgument::Type(t) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

impl<'a> Shape<'a> {
    pub fn of(ty: &'a Type) -> Shape<'a> {
        match ty {
            Type::Group(g) => Shape::of(&g.elem),
            Type::Paren(p) => Shape::of(&p.elem),
            Type::Reference(r) => Shape::Ref {
                mutable: r.mutability.is_some(),
                lifetime: r.lifetime.is_some(),
                inner: &r.elem,
            },
            Type::Slice(s) => Shape::Slice(&s.elem),
            Type::Array(a) => Shape::Array(&a.elem),
            Type::Tuple(t) => Shape::Tuple(t.elems.iter().collect()),
            Type::Path(p) if p.qself.is_none() => {
                let segs: Vec<String> = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                let args: Vec<&GenericArgument> = match p.path.segments.last().map(|s| &s.arguments)
                {
                    Some(PathArguments::AngleBracketed(a)) => a.args.iter().collect(),
                    _ => Vec::new(),
                };
                let tys: Vec<&Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                    .collect();
                wrapper(segs.last().map_or("", String::as_str), &tys)
                    .unwrap_or(Shape::Path { segs, args })
            }
            // The first bound only: `impl Iterator<Item = T> + '_`.
            Type::ImplTrait(i) => i
                .bounds
                .first()
                .and_then(|b| bound_is(b, "Iterator"))
                .and_then(|a| match a {
                    PathArguments::AngleBracketed(a) => match a.args.first() {
                        Some(GenericArgument::AssocType(t)) if t.ident == "Item" => Some(&t.ty),
                        _ => None,
                    },
                    _ => None,
                })
                .map_or(Shape::Other, Shape::ImplIter),
            Type::TraitObject(o) => {
                let mut b = o.bounds.iter();
                let bytes = b
                    .next()
                    .and_then(|b| bound_is(b, "AsRef"))
                    .and_then(one_type)
                    .is_some_and(|t| {
                        matches!(Shape::of(t), Shape::Slice(e) if Shape::of(e).key().as_deref() == Some("u8"))
                    });
                let markers = b.all(|m| match m {
                    TypeParamBound::Lifetime(l) => l.ident == "static",
                    m => ["Send", "Sync"]
                        .iter()
                        .any(|n| bound_is(m, n).is_some_and(PathArguments::is_none)),
                });
                if bytes && markers {
                    Shape::DynBytes
                } else {
                    Shape::Other
                }
            }
            _ => Shape::Other,
        }
    }

    /// The name a registry keys this type by: a path with no arguments,
    /// `crate::` left off (`crate::Foo` and `Foo` are the same type, and
    /// upstream spells some of its own members either way). `filter::Image`
    /// keeps its module.
    pub fn key(&self) -> Option<String> {
        match self {
            Shape::Path { segs, args } if args.is_empty() => {
                let s = segs.strip_prefix(&["crate".to_string()]).unwrap_or(segs);
                Some(s.join("::"))
            }
            _ => None,
        }
    }

    /// The last segment of a path, its arguments aside: `a::B<'_>` is `B`.
    pub fn last(&self) -> Option<&str> {
        match self {
            Shape::Path { segs, .. } => segs.last().map(String::as_str),
            _ => None,
        }
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, Shape::Tuple(t) if t.is_empty())
    }
}

/// [`Shape::key`] of a type.
pub fn key(ty: &Type) -> Option<String> {
    Shape::of(ty).key()
}

/// `ty` with every plain `&` taken off: `&&T` is `T`. A `&mut` or a `&'a`
/// stops it -- what such a reference holds is never read as the value
/// itself, which keeps `Family::Name(&'a str)` reported rather than guessed
/// at.
pub fn unref(ty: &Type) -> &Type {
    match Shape::of(ty) {
        Shape::Ref {
            mutable: false,
            lifetime: false,
            inner,
        } => unref(inner),
        _ => ty,
    }
}

/// The type a member is *about*, its wrappers peeled: a plain `&`,
/// `Result<..>` (its value), `Option<..>`, `Vec<..>`, `Box<..>`, a slice and
/// `impl Iterator<Item = ..>`, as often as they nest. Stops at `Arc<..>`:
/// that marks a shared handle, and peeling it would seed the handle's type
/// as data. The path prefix is left on; callers that want the bare name take
/// it off.
pub fn peel(ty: &Type) -> &Type {
    match Shape::of(ty) {
        Shape::Ref {
            mutable: false,
            lifetime: false,
            inner: t,
        }
        | Shape::Result { ok: t, .. }
        | Shape::Option(t)
        | Shape::Vec(t)
        | Shape::Box(t)
        | Shape::Slice(t)
        | Shape::ImplIter(t) => peel(t),
        _ => ty,
    }
}

/// What a member's type is about, [`peel`]'s answer, or each element's when
/// that is a tuple of two or more: `Vec<(String, Language)>` names both.
pub fn peeled(ty: &Type) -> Vec<&Type> {
    let p = peel(ty);
    match Shape::of(p) {
        Shape::Tuple(elems) if elems.len() > 1 => elems.into_iter().flat_map(peeled).collect(),
        _ => vec![p],
    }
}

/// Normalised textual form of a type, e.g. `Option<std::path::PathBuf>`: what
/// a report line and a [`crate::Config`] entry spell it as. Text only, never
/// read back as a type.
pub fn ty_str(ty: &Type) -> String {
    ty.to_token_stream().to_string().replace(' ', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn name(ty: &Type) -> Option<String> {
        key(ty)
    }

    #[test]
    fn wrappers_by_their_last_segment() {
        let t: Type = parse_quote!(Vec<Option<Box<crate::Foo>>>);
        let Shape::Vec(o) = Shape::of(&t) else {
            panic!("a Vec")
        };
        let Shape::Option(b) = Shape::of(o) else {
            panic!("an Option")
        };
        let Shape::Box(f) = Shape::of(b) else {
            panic!("a Box")
        };
        assert_eq!(name(f).as_deref(), Some("Foo"));
        for t in [
            parse_quote!(Arc<Foo>),
            parse_quote!(std::sync::Arc<Foo>),
            parse_quote!(alloc::sync::Arc<Foo>),
        ] {
            let t: Type = t;
            assert!(matches!(Shape::of(&t), Shape::Arc(_)), "{}", ty_str(&t));
        }
        let t: Type = parse_quote!(Result<(), Error>);
        let Shape::Result { ok, err: Some(_) } = Shape::of(&t) else {
            panic!("a Result")
        };
        assert!(Shape::of(ok).is_unit());
        let t: Type = parse_quote!(Result<Foo>);
        assert!(matches!(Shape::of(&t), Shape::Result { err: None, .. }));
    }

    #[test]
    fn references_say_what_they_are() {
        let t: Type = parse_quote!(&mut [u8]);
        let Shape::Ref {
            mutable: true,
            lifetime: false,
            inner,
        } = Shape::of(&t)
        else {
            panic!("a &mut")
        };
        assert!(matches!(Shape::of(inner), Shape::Slice(_)));
        let t: Type = parse_quote!(&'a str);
        assert!(matches!(Shape::of(&t), Shape::Ref { lifetime: true, .. }));
        // A named lifetime or a `&mut` stops `unref` and `peel`.
        assert_eq!(ty_str(unref(&t)), "&'astr");
        let t: Type = parse_quote!(&&Foo);
        assert_eq!(ty_str(unref(&t)), "Foo");
    }

    #[test]
    fn dyn_bytes_with_markers_in_any_order() {
        for t in [
            parse_quote!(dyn AsRef<[u8]> + Send + Sync),
            parse_quote!(dyn AsRef<[u8]> + Sync + Send),
            parse_quote!(dyn AsRef<[u8]> + Send + Sync + 'static),
            parse_quote!(dyn AsRef<[u8]>),
        ] {
            let t: Type = t;
            assert_eq!(Shape::of(&t), Shape::DynBytes, "{}", ty_str(&t));
        }
        for t in [
            parse_quote!(dyn AsRef<[u16]> + Send),
            parse_quote!(dyn AsRef<[u8]> + Clone),
            parse_quote!(dyn Send + AsRef<[u8]>),
        ] {
            let t: Type = t;
            assert_eq!(Shape::of(&t), Shape::Other, "{}", ty_str(&t));
        }
    }

    #[test]
    fn impl_iterator_names_its_item() {
        let t: Type = parse_quote!(impl Iterator<Item = &Foo> + '_);
        let Shape::ImplIter(item) = Shape::of(&t) else {
            panic!("an iterator")
        };
        assert_eq!(ty_str(item), "&Foo");
        let t: Type = parse_quote!(impl Fn(u8));
        assert_eq!(Shape::of(&t), Shape::Other);
    }

    #[test]
    fn tuples_and_paths() {
        let t: Type = parse_quote!((Vec<(A, B)>, C));
        let Shape::Tuple(e) = Shape::of(&t) else {
            panic!("a tuple")
        };
        assert_eq!(e.len(), 2);
        let t: Type = parse_quote!(());
        assert!(Shape::of(&t).is_unit());
        // A path keys by its segments, `crate` aside, and only without
        // arguments: `Foo<'a>` is not `Foo`.
        let t: Type = parse_quote!(crate::filter::Image);
        assert_eq!(name(&t).as_deref(), Some("filter::Image"));
        let t: Type = parse_quote!(std::path::PathBuf);
        assert_eq!(name(&t).as_deref(), Some("std::path::PathBuf"));
        let t: Type = parse_quote!(Foo<'a>);
        assert_eq!(name(&t), None);
        assert_eq!(Shape::of(&t).last(), Some("Foo"));
        let t: Type = parse_quote!(<T as Trait>::Out);
        assert_eq!(Shape::of(&t), Shape::Other);
    }

    #[test]
    fn peel_takes_the_value() {
        let cases: &[(Type, &str)] = &[
            (parse_quote!(Result<Foo, Error>), "Foo"),
            (parse_quote!(Result<Vec<Foo>, Box<Error>>), "Foo"),
            (parse_quote!(&Option<Box<Foo>>), "Foo"),
            (parse_quote!(impl Iterator<Item = &Vec<Foo>> + '_), "Foo"),
            (parse_quote!(&[Arc<Foo>]), "Arc<Foo>"),
            (parse_quote!(&'a Foo), "&'aFoo"),
        ];
        for (t, want) in cases {
            assert_eq!(ty_str(peel(t)), *want, "{}", ty_str(t));
        }
        let t: Type = parse_quote!(Vec<(String, Option<Language>)>);
        let got: Vec<String> = peeled(&t).into_iter().map(ty_str).collect();
        assert_eq!(got, ["String", "Language"]);
    }
}
