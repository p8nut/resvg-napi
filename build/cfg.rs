// Which upstream methods exist on which target.
//
// Upstream gates items on its own Cargo features (`#[cfg(feature = "memmap")]`),
// and those features are not ours: one this crate turns off for a target makes
// the item vanish there, and a binding that calls it stops compiling. So a
// method's `#[cfg]` -- its own and its impl block's -- is translated before
// emission: each `feature = "x"` becomes what `config::FEATURES` says it is for
// this crate (always on, or on under a cfg of ours), anything else (`unix`,
// `target_os = ..`) is kept as written. A method whose twins together are on
// everywhere is emitted plain, the rest carry the translated cfg.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;

use crate::config::FEATURES;

/// A cfg predicate, features already translated.
#[derive(Clone)]
pub enum Pred {
    True,
    /// Kept as written: `unix`, `target_os = "macos"`, or one of ours.
    Atom(String),
    Not(Box<Pred>),
    All(Vec<Pred>),
    Any(Vec<Pred>),
}

/// What to do with a method, given its cfg.
pub enum Gate {
    Always,
    /// The cfg it is built under, and its negation.
    When(TokenStream, TokenStream),
}

/// The translated predicate of every `#[cfg]` in `attrs`, all of them holding.
pub fn of(attrs: &[syn::Attribute]) -> Pred {
    Pred::All(
        attrs
            .iter()
            .filter(|a| a.path().is_ident("cfg"))
            .map(|a| {
                let meta: syn::Meta = a.parse_args().expect("a #[cfg(..)] whose predicate parses");
                translate(&meta)
            })
            .collect(),
    )
}

fn translate(meta: &syn::Meta) -> Pred {
    match meta {
        syn::Meta::List(l) if ["all", "any", "not"].iter().any(|k| l.path.is_ident(k)) => {
            let inner: Vec<Pred> = l
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .expect("a cfg combinator over predicates")
                .iter()
                .map(translate)
                .collect();
            if l.path.is_ident("all") {
                Pred::All(inner)
            } else if l.path.is_ident("any") {
                Pred::Any(inner)
            } else {
                Pred::Not(Box::new(
                    inner
                        .into_iter()
                        .next()
                        .expect("not(..) takes one predicate"),
                ))
            }
        }
        syn::Meta::NameValue(nv) if nv.path.is_ident("feature") => {
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            else {
                panic!("cfg(feature = ..) without a string");
            };
            let name = s.value();
            match FEATURES.iter().find(|(f, _)| *f == name) {
                Some((_, "true")) => Pred::True,
                Some((_, ours)) => {
                    translate(&syn::parse_str(ours).expect("a config::FEATURES cfg that parses"))
                }
                None => panic!(
                    "an emitted upstream method is gated on feature {name:?}, which config::FEATURES \
                     does not list: say whether this crate always builds it (\"true\") or under \
                     which cfg"
                ),
            }
        }
        other => Pred::Atom(quote!(#other).to_string()),
    }
}

fn atoms(p: &Pred, out: &mut BTreeSet<String>) {
    match p {
        Pred::Atom(a) => {
            out.insert(a.clone());
        }
        Pred::Not(x) => atoms(x, out),
        Pred::All(xs) | Pred::Any(xs) => xs.iter().for_each(|x| atoms(x, out)),
        Pred::True => {}
    }
}

fn holds(p: &Pred, on: &BTreeSet<&String>) -> bool {
    match p {
        Pred::True => true,
        Pred::Atom(a) => on.contains(a),
        Pred::Not(x) => !holds(x, on),
        Pred::All(xs) => xs.iter().all(|x| holds(x, on)),
        Pred::Any(xs) => xs.iter().any(|x| holds(x, on)),
    }
}

fn tokens(p: &Pred) -> TokenStream {
    match p {
        Pred::True => quote!(all()),
        Pred::Atom(a) => a.parse().expect("an atom that was a cfg predicate"),
        Pred::Not(x) => {
            let x = tokens(x);
            quote!(not(#x))
        }
        Pred::All(xs) => {
            let xs = xs.iter().map(tokens);
            quote!(all(#(#xs),*))
        }
        Pred::Any(xs) => {
            let xs = xs.iter().map(tokens);
            quote!(any(#(#xs),*))
        }
    }
}

/// Settles a predicate by truth table over its atoms: they are opaque, so every
/// combination counts as possible. ponytail: 2^atoms, fine for the handful a
/// method carries; a cfg with dozens of distinct atoms would want a real solver.
pub fn gate(p: &Pred) -> Gate {
    let mut set = BTreeSet::new();
    atoms(p, &mut set);
    let all: Vec<&String> = set.iter().collect();
    assert!(all.len() < 16, "a cfg with {} atoms", all.len());
    let always = (0..1u32 << all.len()).all(|bits| {
        let on: BTreeSet<&String> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, a)| *a)
            .collect();
        holds(p, &on)
    });
    if always {
        Gate::Always
    } else {
        Gate::When(tokens(p), tokens(&Pred::Not(Box::new(p.clone()))))
    }
}
