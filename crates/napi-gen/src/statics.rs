//! Associated constants (`impl T { pub const X: .. }`) as static read-only
//! properties, under their Rust name (ADR 0002, Q21): `T.X`.
//!
//! - On a class ([`of_class`]): a constant of a number, boolean or string
//!   type reads as that value; one of type `Self`, on a class held by value,
//!   reads as a fresh instance on each read, so writing to it changes no
//!   other read.
//! - On a type with no class in JS ([`of_values`]), a newtype JS reads as a
//!   number, say: a frozen object of the same name in the namespace, its
//!   values converted as that type's members are.
//!
//! napi declares no static property. A class's constants are hidden static
//! methods, which a `#[napi(module_exports)]` hook ([`hook`]) turns into
//! getters when the module loads; the same hook builds the objects. Both
//! are declared in the `.d.ts` header (napi's `dtsHeaderFile`), merged into
//! the class by a namespace of its name, or as a `const`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ImplItem, ImplItemConst, Item, ItemImpl, Type};

use crate::class::{Classes, Holding};
use crate::emit::{Site, class_of, const_type, js_conv};
use crate::shape::key;
use crate::sources::{ModTree, inherent_impls, is_pub, self_ident};
use crate::vocab::{Vocab, docs, ty_str, upstream_path};

/// One associated constant given a JS form: `ns.owner.name`.
pub struct Static {
    pub ns: String,
    /// The type's JS name: its class, or the object standing for it.
    pub owner: String,
    pub name: String,
    /// Its TypeScript type.
    pub ts: String,
    /// Its doc comment, line by line.
    pub doc: Vec<String>,
    /// On an object, the value as JS. `None` on a class: the hidden static
    /// method of its name is the getter.
    pub value: Option<TokenStream>,
}

/// The public constants of `ty`'s inherent impls in `files`, each with
/// whether it or its impl is behind a `#[cfg]`.
fn consts_of<'f>(
    files: &'f [syn::File],
    ty: &'f str,
) -> impl Iterator<Item = (&'f ImplItemConst, bool)> {
    inherent_impls(files, ty).flat_map(|imp| {
        let gated = is_gated(&imp.attrs);
        imp.items.iter().filter_map(move |it| match it {
            ImplItem::Const(k) if is_pub(&k.vis) => Some((k, gated || is_gated(&k.attrs))),
            _ => None,
        })
    })
}

fn is_gated(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg"))
}

/// A doc comment's lines, without the space rustdoc puts first.
fn doc_lines(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }),
                ..
            }) => Some(s.value()),
            _ => None,
        })
        .flat_map(|s| {
            s.lines()
                .map(|l| l.strip_prefix(' ').unwrap_or(l).to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The associated constants of class `c`: a `#[napi] impl` of a hidden
/// static method each, what they are, and a report line each.
pub fn of_class(
    classes: &Classes,
    c: usize,
    vocab: &Vocab,
) -> (TokenStream, Vec<Static>, Vec<String>) {
    let class = &classes.classes[c];
    let src = &classes.sources[class.source];
    let (name, root) = (&class.name, src.root);
    let id = classes.ident(c);
    let up = classes.path(class.source, name);
    let (mut methods, mut out, mut report) = (TokenStream::new(), Vec::new(), Vec::new());
    for (k, gated) in consts_of(src.files, name) {
        let at = format!("{root}::{name}::{}", k.ident);
        let Some(ns) = classes.config.namespace_of(root) else {
            report.push(format!(
                "{at} constant not exposed: no namespace for {root}"
            ));
            continue;
        };
        if gated {
            report.push(format!("{at} constant not exposed: behind a #[cfg]"));
            continue;
        }
        let own = matches!(key(&k.ty).as_deref(), Some(t) if t == "Self" || t == name);
        let (ret, body, ts, napi) = if own {
            if class.holding != Holding::Value {
                report.push(format!(
                    "{at} constant not exposed: {name} is not held by value, so no copy per read"
                ));
                continue;
            }
            // Built from the class it is read on, as a factory is: napi
            // looks a returned class up by JS name, which another
            // namespace shares (`Class::shared_name`).
            let napi = if class.shared_name {
                quote!(factory,)
            } else {
                TokenStream::new()
            };
            let k = &k.ident;
            (
                quote!(Self),
                quote!(Self::__from(#up::#k)),
                name.clone(),
                napi,
            )
        } else if let Some((ty, ts)) = const_type(&k.ty, vocab) {
            // A function returning a borrow names its lifetime.
            let ty = match ts {
                "string" if ty.to_string().starts_with('&') => quote!(&'static str),
                _ => ty,
            };
            let k = &k.ident;
            (ty, quote!(#up::#k), ts.to_string(), TokenStream::new())
        } else {
            report.push(format!(
                "{at} constant not exposed: {} (no JS value)",
                ty_str(&k.ty)
            ));
            continue;
        };
        let (ident, js) = (&k.ident, k.ident.to_string());
        let doc = docs(&k.attrs);
        // Configurable, which napi's statics are not: the hook redefines it.
        methods.extend(quote! {
            #(#doc)*
            #[napi(#napi js_name = #js, configurable = true, skip_typescript)]
            #[allow(non_snake_case)]
            pub fn #ident() -> #ret {
                #body
            }
        });
        report.push(format!("{at} is a static: {ns}.{name}.{js}"));
        out.push(Static {
            ns: ns.to_string(),
            owner: name.clone(),
            name: js,
            ts,
            doc: doc_lines(&k.attrs),
            value: None,
        });
    }
    let code = if methods.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            #[napi]
            impl #id {
                #methods
            }
        }
    };
    (code, out, report)
}

/// The associated constants of a discovered crate's types that are not
/// classes (`listed` says which are), read from its `parsed` files: each
/// type JS reads as a number, boolean or string gets an object of its name
/// in `ns`, holding its constants converted as its members are. Another
/// type's constants are reported, as is a type no export reaches.
pub fn of_values(
    root_str: &str,
    root: &TokenStream,
    parsed: &[(PathBuf, syn::File)],
    tree: &ModTree,
    listed: impl Fn(&str) -> bool,
    vocab: &Vocab,
) -> (Vec<Static>, Vec<String>) {
    let (mut out, mut report) = (Vec::new(), Vec::new());
    let ns = vocab.config.namespace_of(root_str);
    let files: Vec<syn::File> = parsed.iter().map(|(_, f)| f.clone()).collect();
    // Each type once, in source order.
    let mut seen = BTreeSet::new();
    for (_, file) in parsed {
        for item in &file.items {
            let Item::Impl(imp) = item else { continue };
            let Some(ty) = owner(imp) else { continue };
            // A type no export reaches has no path to read it by. By name:
            // ponytail, a private type sharing an exported one's name has
            // its impls read with it; none has a constant.
            if listed(&ty) || !tree.paths.contains_key(&ty) || !seen.insert(ty.clone()) {
                continue;
            }
            for (k, gated) in consts_of(&files, &ty) {
                let at = format!("{root_str}::{ty}::{}", k.ident);
                let Some(ns) = ns else {
                    report.push(format!(
                        "{at} constant not exposed: no namespace for {root_str}"
                    ));
                    continue;
                };
                if gated {
                    report.push(format!("{at} constant not exposed: behind a #[cfg]"));
                    continue;
                }
                let up = upstream_path(&ty, &tree.paths, root);
                let id = &k.ident;
                let access = quote!(#up::#id);
                // `Self` is the type by name.
                let own: Type = match key(&k.ty).as_deref() {
                    Some("Self") => syn::parse_str(&ty).expect("a type name"),
                    _ => k.ty.clone(),
                };
                let value = match const_type(&own, vocab) {
                    Some((_, ts)) => Some((access, ts)),
                    // Converted as a member of the type is.
                    None => vocab
                        .classify(&own)
                        .and_then(|js| js_conv(&js, &own, vocab, &access, Site::Field))
                        .and_then(|conv| {
                            let ts = match conv.ty.to_string().as_str() {
                                "u32" | "i32" | "f64" => "number",
                                "bool" => "boolean",
                                "String" => "string",
                                _ => return None,
                            };
                            Some((conv.expr, ts))
                        }),
                };
                let Some((value, ts)) = value else {
                    report.push(format!(
                        "{at} constant not exposed: {} (no JS value)",
                        ty_str(&own)
                    ));
                    continue;
                };
                let name = k.ident.to_string();
                report.push(format!("{at} is a static: {ns}.{ty}.{name}"));
                out.push(Static {
                    ns: ns.to_string(),
                    owner: ty.clone(),
                    name,
                    ts: ts.to_string(),
                    doc: doc_lines(&k.attrs),
                    value: Some(value),
                });
            }
        }
    }
    (out, report)
}

/// The type an inherent impl is of, when it names a plain one.
fn owner(imp: &ItemImpl) -> Option<String> {
    if imp.trait_.is_some() || !imp.generics.params.is_empty() {
        return None;
    }
    self_ident(imp).map(ToString::to_string)
}

/// The associated constants of a Role::Classes crate's types that are not
/// its classes, as `Type::NAME`: read only for those classes, it emits none
/// of them.
pub fn unexposed(
    files: &[syn::File],
    exported: &BTreeSet<String>,
    listed: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for item in files.iter().flat_map(|f| &f.items) {
        let Item::Impl(imp) = item else { continue };
        let Some(ty) = owner(imp) else { continue };
        if listed(&ty) || !exported.contains(&ty) || !seen.insert(ty.clone()) {
            continue;
        }
        out.extend(consts_of(files, &ty).map(|(k, _)| format!("{ty}::{}", k.ident)));
    }
    out
}

/// What the statics need once the file is complete: `define_statics`,
/// which the `#[napi(module_exports)]` hook calls (napi runs one,
/// `generate`'s), turning each class's hidden methods into getters and
/// building the objects, and their `.d.ts` declarations. A
/// static whose class the file lost (`namespaced` leaves out what names a
/// left-out item), or whose object's name a class of its namespace has, is
/// dropped, with a report line.
pub fn hook(file: &syn::File, statics: Vec<Static>) -> (TokenStream, String, Vec<String>) {
    let classes: BTreeSet<(String, String)> = file
        .items
        .iter()
        .filter_map(class_of)
        .filter_map(|(_, js, ns)| Some((ns?, js)))
        .collect();
    let mut report = Vec::new();
    // By namespace, then owner: the class getters, and the objects.
    let mut getters: BTreeMap<(String, String), Vec<Static>> = BTreeMap::new();
    let mut objects: BTreeMap<(String, String), Vec<Static>> = BTreeMap::new();
    for s in statics {
        let at = (s.ns.clone(), s.owner.clone());
        let class = classes.contains(&at);
        match (&s.value, class) {
            (None, true) => getters.entry(at).or_default().push(s),
            (Some(_), false) => objects.entry(at).or_default().push(s),
            (None, false) => report.push(format!(
                "{}.{}.{} not exposed: no class {}.{} in the file",
                s.ns, s.owner, s.name, s.ns, s.owner
            )),
            (Some(_), true) => report.push(format!(
                "{}.{}.{} not exposed: {}.{} is a class already",
                s.ns, s.owner, s.name, s.ns, s.owner
            )),
        }
    }
    if getters.is_empty() && objects.is_empty() {
        return (TokenStream::new(), String::new(), report);
    }
    let rows = getters.iter().map(|((ns, class), statics)| {
        let names = statics.iter().map(|s| &s.name);
        quote!((#ns, #class, &[#(#names),*]))
    });
    let built = objects.iter().map(|((ns, owner), statics)| {
        let sets = statics.iter().map(|s| {
            let (name, value) = (&s.name, s.value.as_ref().expect("an object's value"));
            quote!(object.set_named_property(#name, #value)?;)
        });
        quote! {
            {
                let mut object = Object::new(env)?;
                #(#sets)*
                object.freeze()?;
                napi_support::namespace(env, &exports, #ns)?.set_named_property(#owner, object)?;
            }
        }
    });
    let code = quote! {
        #[doc = " The associated constants, static read-only: each class's hidden static"]
        #[doc = " methods become getters, and a type with no class gets a frozen object of"]
        #[doc = " its name. The `.d.ts` header declares them."]
        fn define_statics(env: &Env, exports: Object) -> Result<()> {
            let classes: &[(&str, &str, &[&str])] = &[#(#rows),*];
            for (ns, class, names) in classes {
                let class: Object = napi_support::namespace(env, &exports, ns)?
                    .get_named_property_unchecked(class)?;
                napi_support::static_getters(env, &class, names)?;
            }
            #(#built)*
            Ok(())
        }
    };
    let mut header = String::from(
        "\n// @generated by napi-gen: the associated constants, static read-only.\n\
         // DO NOT EDIT. Set when the module loads.\n",
    );
    let namespaces: BTreeSet<&String> = getters
        .keys()
        .chain(objects.keys())
        .map(|(ns, _)| ns)
        .collect();
    for ns in namespaces {
        header.push_str(&format!("\nexport declare namespace {ns} {{\n"));
        for ((_, class), statics) in getters.iter().filter(|((n, _), _)| n == ns) {
            header.push_str(&format!("  export namespace {class} {{\n"));
            for s in statics {
                jsdoc(&mut header, "    ", &s.doc);
                header.push_str(&format!("    export const {}: {}\n", s.name, s.ts));
            }
            header.push_str("  }\n");
        }
        for ((_, owner), statics) in objects.iter().filter(|((n, _), _)| n == ns) {
            header.push_str(&format!("  export const {owner}: {{\n"));
            for s in statics {
                jsdoc(&mut header, "    ", &s.doc);
                header.push_str(&format!("    readonly {}: {}\n", s.name, s.ts));
            }
            header.push_str("  }\n");
        }
        header.push_str("}\n");
    }
    (code, header, report)
}

/// `doc` as a JSDoc comment at `indent`, as napi writes one.
fn jsdoc(out: &mut String, indent: &str, doc: &[String]) {
    match doc {
        [] => {}
        [line] => out.push_str(&format!("{indent}/** {line} */\n")),
        lines => {
            out.push_str(&format!("{indent}/**\n"));
            for l in lines {
                if l.is_empty() {
                    out.push_str(&format!("{indent} *\n"));
                } else {
                    out.push_str(&format!("{indent} * {l}\n"));
                }
            }
            out.push_str(&format!("{indent} */\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(owner: &str, name: &str, value: Option<TokenStream>) -> Static {
        Static {
            ns: "ns".into(),
            owner: owner.into(),
            name: name.into(),
            ts: "number".into(),
            doc: vec!["One.".into(), String::new(), "Two.".into()],
            value,
        }
    }

    #[test]
    fn the_hook_declares_what_the_file_can_hold() {
        let file: syn::File = syn::parse_quote! {
            #[napi(js_name = "Color", namespace = "ns")]
            pub struct NsColor { inner: u8 }
        };
        let (code, header, report) = hook(
            &file,
            vec![
                stat("Color", "MAX", None),
                stat("Gone", "MAX", None),
                stat("Weight", "BOLD", Some(quote!(700u32))),
                stat("Color", "MIN", Some(quote!(0u32))),
            ],
        );
        assert!(code.to_string().contains("fn define_statics"));
        assert_eq!(
            report,
            [
                "ns.Gone.MAX not exposed: no class ns.Gone in the file",
                "ns.Color.MIN not exposed: ns.Color is a class already",
            ]
        );
        assert!(header.contains(
            "export declare namespace ns {\n  export namespace Color {\n    /**\n     * One.\n     \
             *\n     * Two.\n     */\n    export const MAX: number\n  }\n"
        ));
        assert!(header.contains("  export const Weight: {\n"));
        assert!(header.contains("    readonly BOLD: number\n  }\n}\n"));
    }

    #[test]
    fn no_static_no_hook() {
        let (code, header, report) = hook(&syn::parse_quote!(), Vec::new());
        assert!(code.is_empty() && header.is_empty() && report.is_empty());
    }
}
