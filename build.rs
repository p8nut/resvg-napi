//! Generates the whole NAPI binding layer from the *actual* resvg / usvg /
//! fontdb sources: `syn` parses them, `quote` re-emits Rust.
//!
//! One phase per file, and this one orchestrates them:
//!
//!   build/config.rs    the resvg-specific lists: renames, reserved names,
//!                      hand-wired fields. What another crate would rewrite
//!   build/sources.rs   finding and parsing upstream
//!                      (env override -> git submodule -> cargo registry)
//!   build/vocab.rs     what a Rust type means on the JS side -- newtypes,
//!                      aliases, `Deref` chains, payload enums, naming
//!   build/emit.rs      turning those answers into tokens napi can expand
//!   build/template.rs  the API decisions, written by hand. With config.rs,
//!                      the only files here not describing upstream
//!   build.rs           this: the passes, their order, the fixpoints, the
//!                      assertions, and writing src/lib.rs
//!
//! The split is verifiable rather than a matter of taste: src/lib.rs,
//! index.d.ts and codegen-report.txt are committed and CI diffs them, so a
//! refactor here is correct exactly when the output does not move.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, ImplItem, Item};

/// The codegen's own report: what it derived from upstream, and what it left
/// alone with the reason. Cargo gives a build script no channel but
/// `cargo::warning`, so this competes with real warnings -- clippy's four were
/// buried under 101 lines of it. Off unless asked for.
macro_rules! report {
    ($($arg:tt)*) => {
        if env::var_os($crate::codegen_log_var()).is_some() {
            println!("cargo::warning={}", format!($($arg)*));
        }
    };
}

/// `RESVG_NAPI_CODEGEN_LOG` for this package: the crate name, upper-cased.
fn codegen_log_var() -> String {
    let pkg = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME, set by Cargo");
    format!("{}_CODEGEN_LOG", pkg.to_uppercase().replace('-', "_"))
}

// Each phase in its own file. `#[path]` because Cargo compiles this one as a
// crate root, so a plain `mod` would look for the file beside it -- in the
// repository root.
#[path = "build/cfg.rs"]
mod cfg;
#[path = "build/config.rs"]
mod config;
#[path = "build/emit.rs"]
mod emit;
#[path = "build/sources.rs"]
mod sources;
#[path = "build/template.rs"]
mod template;
#[path = "build/vocab.rs"]
mod vocab;

use config::*;
use emit::*;
use sources::*;
use template::*;
use vocab::*;

// ---------------------------------------------------------------------------
// 1. source discovery
// ---------------------------------------------------------------------------

fn assert_free_fn(files: &[syn::File], name: &str, want: &[&str]) {
    let f = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Fn(f) if f.sig.ident == name && is_pub(&f.vis) => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("resvg: `pub fn {name}` disappeared"));
    let got: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|a| match a {
            syn::FnArg::Typed(t) => ty_str(&t.ty),
            syn::FnArg::Receiver(_) => "self".into(),
        })
        .collect();
    assert_eq!(
        got,
        want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "resvg::{name} signature changed; update build.rs"
    );
}

/// The emitter templates code against these fields by name; break loudly if they move.
fn assert_struct_fields(files: &[syn::File], ty: &str, want: &[&str]) {
    let s = files
        .iter()
        .flat_map(|f| &f.items)
        .find_map(|i| match i {
            Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("usvg: `pub struct {ty}` not found"));
    let Fields::Named(named) = &s.fields else {
        panic!("usvg::{ty} is no longer a struct with named fields")
    };
    for w in want {
        assert!(
            named
                .named
                .iter()
                .any(|f| f.ident.as_ref().is_some_and(|i| i == w)),
            "usvg::{ty}::{w} disappeared; update build.rs"
        );
    }
}

fn assert_tree_ctor(files: &[syn::File], ty: &str, name: &str) {
    let found = files.iter().flat_map(|f| &f.items).any(|i| match i {
        // upstream writes `impl crate::Tree`, so compare the last path segment
        Item::Impl(imp) if ty_str(&imp.self_ty).rsplit("::").next() == Some(ty) => imp
            .items
            .iter()
            .any(|it| matches!(it, ImplItem::Fn(f) if f.sig.ident == name && is_pub(&f.vis))),
        _ => false,
    });
    assert!(found, "upstream: `{ty}::{name}` disappeared");
}

// ---------------------------------------------------------------------------
// 4. emission
// ---------------------------------------------------------------------------

fn main() {
    napi_build::setup();
    let probe = quote!(image_kind_to_js(x));
    assert!(
        calls(&probe, "image_kind_to_js") && !calls(&probe, "kind_to_js"),
        "calls() matches inside a longer identifier"
    );
    // map_struct arms no upstream option struct exercises today, so a
    // regression in them would only show the day one does: a struct with a
    // `Rect`, a `NonZeroRect`, an `Option<NonZeroRect>`, a renamed enum and an
    // integer newtype, mapped and read back as tokens.
    {
        let probe: syn::File = syn::parse_quote! {
            pub struct Probe { pub r: Rect, pub n: NonZeroRect, pub o: Option<NonZeroRect>, pub e: Style, pub i: ID }
        };
        let mut v = Vocab::default();
        v.enums.insert("Style".into());
        v.ints.insert("ID".into());
        let (fields, _, _) = map_struct(&[probe], "Probe", &v, 0);
        let got: Vec<String> = fields
            .iter()
            .map(|f| format!("{} {}", f.decl, f.assign).replace(' ', ""))
            .collect();
        assert!(
            got[0].contains("usvg::Rect::from_xywh") && got[0].contains("o.r=r;"),
            "Rect: {}",
            got[0]
        );
        assert!(
            got[1].contains("usvg::NonZeroRect::from_xywh") && got[1].contains("o.n=r;"),
            "NonZeroRect: {}",
            got[1]
        );
        assert!(
            got[2].contains("o.o=Some(r);"),
            "Option<NonZeroRect>: {}",
            got[2]
        );
        assert!(
            got[3].contains("Option<FontFaceStyle>"),
            "renamed enum: {}",
            got[3]
        );
        assert!(
            got[4].contains("try_into()") && !got[4].contains("as_"),
            "int newtype: {}",
            got[4]
        );
    }
    cfg::selftest();
    modtree_selftest(Path::new(
        &env::var("OUT_DIR").expect("OUT_DIR, set by Cargo for build scripts"),
    ));
    // map_methods: a name skipped on one pass stays free for the next, and a
    // `where` bound classifies its argument like an inline one does.
    {
        let a: syn::File = syn::parse_quote! { impl A { pub fn foo(self) {} } };
        let b: syn::File = syn::parse_quote! {
            impl B {
                pub fn foo(&self) -> u32 { 0 }
                pub fn load<P>(&mut self, p: P) where P: AsRef<Path> {}
            }
        };
        let (v, recv, mut taken) = (Vocab::default(), quote!(self.inner), BTreeSet::new());
        let pass = |files: &[syn::File], ty: &str, taken: &mut BTreeSet<String>| {
            map_methods(
                &MethodPass {
                    files,
                    ty,
                    receiver: &recv,
                    skip: &[],
                    prologue: None,
                    readonly: false,
                    optional: false,
                },
                &v,
                taken,
            )
        };
        let (_, skipped) = pass(&[a], "A", &mut taken);
        assert!(
            skipped.iter().any(|s| s.name == "foo"),
            "pass 1 skips foo(self)"
        );
        let (code, skipped) = pass(&[b], "B", &mut taken);
        let code = code.to_string().replace(' ', "");
        assert!(code.contains("pubfnfoo("), "pass 2 still emits foo: {code}");
        assert!(
            code.contains("pubfnload(&mutself,p:String)"),
            "where-bound arg: {code}"
        );
        assert!(skipped.is_empty(), "nothing skipped on pass 2");
        let ne: syn::ItemEnum = syn::parse_quote! { #[non_exhaustive] pub enum E { A } };
        assert!(
            payload_shape(&ne, &|t| t).is_err(),
            "#[non_exhaustive] is refused"
        );
    }
    // One table: a kind that both a getter and a method can return has the
    // same JS type at both, whatever expression each uses to get there.
    {
        let (v, a) = (Vocab::default(), quote!(x));
        for js in [
            Js::Bool,
            Js::I32,
            Js::F32,
            Js::F64,
            Js::U8,
            Js::U16,
            Js::U32,
            Js::Count,
            Js::Str,
            Js::Scalar,
            Js::Bbox,
            Js::Size,
            Js::Matrix,
            Js::IntNewtype("N".into()),
            Js::Value("V".into()),
            Js::ValueList("V".into()),
            Js::OptValue("V".into()),
            Js::Object("O".into()),
            Js::ObjectList("O".into()),
        ] {
            let ty = |site| js_conv(&js, "T", &v, &a, site).map(|c| c.ty.to_string());
            assert_eq!(
                ty(Site::Getter),
                ty(Site::Method),
                "{js:?}: getter and method disagree"
            );
        }
    }
    for (ty, peeled) in [
        ("&[Arc<ClipPath>]", "Arc<ClipPath>"),
        ("Option<&Vec<Stop>>", "Stop"),
        ("implIterator<Item=&Node>+'_", "Node"),
        ("&[TextChunk]", "TextChunk"),
        ("Box<Group>", "Group"),
        ("usvg::Color", "usvg::Color"),
    ] {
        assert_eq!(peel_type(ty), peeled, "peel_type({ty})");
    }
    println!("cargo::rerun-if-changed=build.rs");
    // The generator is six files now, and Cargo watches only what it is told:
    // without these, editing a phase module would leave src/lib.rs stale and the
    // CI drift check would fail on someone else's machine rather than here.
    for m in ["cfg", "config", "sources", "vocab", "emit", "template"] {
        println!("cargo::rerun-if-changed=build/{m}.rs");
    }
    // The generated file is also an *input* to watch: if someone deletes or
    // stubs src/lib.rs, the script must re-run and regenerate it.
    println!("cargo::rerun-if-changed=src/lib.rs");
    println!("cargo::rerun-if-env-changed={}", codegen_log_var());

    let locked = locked_versions();

    // The crates this generator derives from (config::UPSTREAM), in the order
    // a bare name is resolved against them. The point of the list is that a
    // third is a row rather than another arm in five `if` chains -- the bare
    // name decided the crate in every one of them, which is the assumption
    // that breaks the moment two crates define `Color`.
    //
    // Whole crates, no per-file list: the `impl` blocks we wrap are spread over
    // tree/mod.rs, tree/filter.rs, parser/, writer.rs... Read through each
    // crate's own module tree: which types are public, and at which path, is
    // what its `mod`s and `pub use`s say.
    //
    // `label` is not `root.to_string()`: quote! spells a path with spaces
    // around `::`, and this one reaches the report a reader is expected to
    // grep.
    struct Source {
        files: Vec<syn::File>,
        parsed: Vec<(PathBuf, syn::File)>,
        tree: ModTree,
        root: TokenStream,
        root_str: &'static str,
        label: &'static str,
    }
    let sources: Vec<Source> = UPSTREAM
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let dir = locate(u.pkg, u.marker, &locked);
            if i == 0 {
                report!("{} sources: {}", u.label, dir.display());
            }
            let all = parse_crate(&dir);
            let tree = ModTree::load(&dir, &all);
            let parsed = tree.public_view(&all, &dir);
            Source {
                files: parsed.iter().map(|(_, f)| f.clone()).collect(),
                parsed,
                tree,
                root: u
                    .root
                    .parse()
                    .expect("a config::UPSTREAM root that is a path"),
                root_str: u.root,
                label: u.label,
            }
        })
        .collect();
    // The primary crate: options, guards and clamps are read from it.
    let primary = &sources[0];
    let primary_files = primary.files.clone();
    let modules = primary.tree.paths.clone();
    // Its files grouped by the namespace they export into. A bare-name lookup
    // over the whole crate takes whichever file was parsed first, which is how
    // `filter::Image` came back with `usvg::Image`'s methods; a qualified key
    // resolves against its own module instead.
    let mut by_module: BTreeMap<String, Vec<syn::File>> = BTreeMap::new();
    for (path, file) in &primary.parsed {
        let m = primary.tree.file_ns.get(path).cloned().unwrap_or_default();
        by_module.entry(m).or_default().push(file.clone());
    }
    // Only the crate root for free functions: a module-level `pub fn` elsewhere
    // is not crate-public (resvg has an internal `render(&Image, ...)`).
    let renderer_root = vec![parse(
        &locate(RENDERER.0, RENDERER.1, &locked).join(RENDERER.1),
    )];
    let source_files = |label: &str| {
        &sources
            .iter()
            .find(|s| s.label == label)
            .unwrap_or_else(|| panic!("no config::UPSTREAM labelled {label:?}"))
            .files
    };

    // Which crate defines a type, by the same probe the chains used: the first
    // source with a public struct of that name.
    let defines = |t: &str| {
        sources
            .iter()
            .find(|s| struct_fields_opt(&s.files, bare(t)).is_some())
    };

    // Enums count as well as structs, and they count together: `payload_enums`
    // qualifies a carried type by this set, so an enum defined in two modules --
    // or an enum in one and a struct of the same name in another -- would be
    // keyed by its bare name and resolve to whichever the walk reached first.
    // That is the failure `filter::Image` already caused once on the struct side.
    let dups: BTreeSet<String> = {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        // Across every source, not within usvg alone. The count is what decides
        // whether a name is keyed by its module, and a name defined once in
        // each of two crates resolves to whichever was walked first -- the same
        // failure `filter::Image` caused inside usvg, one level up. Nothing
        // shares a name between usvg and fontdb today; tiny-skia would bring
        // nine at once, `Color` and `Paint` among them.
        for f in sources.iter().flat_map(|s| s.files.iter()) {
            for item in &f.items {
                let name = match item {
                    Item::Struct(st) if is_pub(&st.vis) => st.ident.to_string(),
                    Item::Enum(e) if is_pub(&e.vis) => e.ident.to_string(),
                    _ => continue,
                };
                *seen.entry(name).or_default() += 1;
            }
        }
        seen.into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(k, _)| k)
            .collect()
    };
    if !dups.is_empty() {
        report!(
            "names defined more than once across the sources, keyed by module: {}",
            dups.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    // Guards first: no point generating against a moved API.
    assert_free_fn(&renderer_root, RENDER_FN.0, RENDER_FN.1);
    for m in TREE.1 {
        assert_tree_ctor(&primary_files, TREE.0, m);
    }
    for (ty, fields) in HAND_FILLED {
        assert_struct_fields(&primary_files, ty, fields);
    }

    // The table has N entries, so the highest usable index is N - 1.
    let precision_max = u32::try_from(static_array_len(&primary_files, CLAMP.1) - 1)
        .expect("a clamp table that fits u32");
    report!(
        "{} precision clamp derived from {}: {precision_max}",
        primary.label,
        CLAMP.1
    );

    // Enums to mirror = named by a config field OR by a return of a wrapped impl.
    let mut referenced = struct_field_types(&primary_files, PARSE_OPTIONS);
    referenced.extend(struct_field_types(&primary_files, WRITE_OPTIONS));
    referenced.extend(returned_types(&primary_files, &[]));
    for src in &sources[1..] {
        referenced.extend(returned_types(&src.files, &[]));
        referenced.extend(field_types(&src.files));
    }

    // Object candidates: a *collection* of values wants a value type, whereas a
    // single borrow wants a handle. So `&[Stop]` and `impl Iterator<Item=&FaceInfo>`
    // make Stop/FaceInfo objects, while `&Group` stays a handle question.
    // Everything asked of the template is written in its own body, so the
    // fragments can be blank. Reused by the pruner further down.
    let nothing = TokenStream::new();
    let probe = template(&Fragments::probe(), &nothing, &nothing, &nothing, &nothing);
    // The names the generated code spells and the template must define. A
    // template edit that drops one fails here, by name, instead of as a
    // compile error deep in src/lib.rs.
    {
        let file: syn::File = syn::parse2(probe.clone()).expect("the probe template parses");
        let mut defined: BTreeSet<String> = file
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Struct(x) => Some(x.ident.to_string()),
                Item::Enum(x) => Some(x.ident.to_string()),
                Item::Fn(x) => Some(x.sig.ident.to_string()),
                Item::Trait(x) => Some(x.ident.to_string()),
                Item::Type(x) => Some(x.ident.to_string()),
                _ => None,
            })
            .collect();
        // In scope through an import counts too: `Signal` comes from
        // napi-support.
        fn imported(t: &syn::UseTree, out: &mut BTreeSet<String>) {
            match t {
                syn::UseTree::Path(p) => imported(&p.tree, out),
                syn::UseTree::Name(n) => {
                    out.insert(n.ident.to_string());
                }
                syn::UseTree::Rename(r) => {
                    out.insert(r.rename.to_string());
                }
                syn::UseTree::Group(g) => g.items.iter().for_each(|i| imported(i, out)),
                syn::UseTree::Glob(_) => {}
            }
        }
        for item in &file.items {
            if let Item::Use(u) = item {
                imported(&u.tree, &mut defined);
            }
        }
        let missing: Vec<_> = TEMPLATE_CONTRACT
            .iter()
            .filter(|n| !defined.contains(**n))
            .collect();
        assert!(
            missing.is_empty(),
            "the template no longer defines {missing:?} (config::TEMPLATE_CONTRACT)"
        );
    }
    let object_seeds: BTreeSet<String> = referenced
        .iter()
        .filter_map(|t| {
            let b = t.trim_start_matches('&');
            if let Some(inner) = b.strip_prefix("[").and_then(|s| s.strip_suffix("]")) {
                return Some(inner.trim_start_matches('&').to_string());
            }
            if let Some(rest) = b.strip_prefix("implIterator<Item=") {
                return Some(rest.split('>').next()?.trim_start_matches('&').to_string());
            }
            None
        })
        // Plus what a payload enum carries. `filter::Kind` names seventeen
        // primitive structs and nothing else does. Safe now on two counts: a
        // duplicated name is keyed by its module, and a member returning a node
        // type no longer drags the tree in behind it.
        .chain(
            by_module
                .iter()
                .flat_map(|(m, files)| payload_enums(files, m, &dups, primary.root_str))
                .flat_map(|(_, p)| p.variants)
                .flat_map(|(_, payload)| match payload {
                    Payload::None => Vec::new(),
                    Payload::Value(t) => vec![t],
                    Payload::Fields(f) => f.into_iter().map(|(_, t)| t).collect(),
                })
                .filter(|t| !NODE_TYPES.contains(&bare(t)))
                .filter(|t| struct_fields_opt(&primary_files, bare(t)).is_some()),
        )
        // Plus whatever the template hands out itself.
        .chain(template_returns(&probe).into_iter().filter(|t| {
            // A real upstream struct, and not one already claimed as a handle:
            // `clipPath()` returns the *wrapper class* ClipPath, which shares
            // its name with usvg::ClipPath. Seeding it would emit `wrap` twice.
            let arc_held = referenced.iter().any(|r| {
                r.trim_start_matches('&')
                    .trim_start_matches('[')
                    .strip_prefix("Arc<")
                    .and_then(|i| i.split('>').next())
                    == Some(t.as_str())
            });
            !arc_held
                && sources
                    .iter()
                    .any(|s| struct_fields_opt(&s.files, t).is_some())
        }))
        .collect();

    let mut vocab = Vocab::default();
    let extra: Vec<syn::File> = VOCAB_CRATES
        .iter()
        .flat_map(|c| parse_crate(&locate(c, "lib.rs", &locked)))
        .map(|(_, f)| f)
        .collect();
    // Each set with the crate path its types are reached by. `extra` is mined
    // for vocabulary only -- what usvg re-exports from tiny-skia-path is
    // reachable as `usvg::Transform`, and what it does not re-export is never
    // emitted, so `usvg` is the honest label for the half that can appear.
    for (set, root) in sources
        .iter()
        .map(|s| (&s.files, s.root_str))
        .chain([(&extra, VOCAB_ROOT)])
    {
        // tiny-skia-path is parsed for exactly this kind of answer: usvg
        // re-exports its `Transform`, so the struct that decides the mapping is
        // never in usvg's own files.
        vocab.matrices.extend(matrix_like(set));
        vocab.scalars.extend(f32_newtypes(set));
        vocab.aliases.extend(type_aliases(set));
        vocab.payload.extend(payload_enums(set, "", &dups, root));
        for item in set.iter().flat_map(|f| &f.items) {
            if let Item::Struct(st) = item
                && is_pub(&st.vis)
            {
                vocab.structs.insert(st.ident.to_string());
            }
        }
        vocab.with_id.extend(types_with_id(set));
    }
    // Rescanned per module so an ambiguous payload gets its module: the pass
    // above keyed everything bare, which is right for the crates that have no
    // duplicate and wrong for the one that does.
    for (m, files) in &by_module {
        if m.is_empty() {
            continue;
        }
        vocab
            .payload
            .extend(payload_enums(files, m, &dups, primary.root_str));
    }
    report!(
        "payload enums found: {}",
        vocab
            .payload
            .iter()
            .map(|(n, p)| format!("{n}({})", p.variants.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    report!(
        "vocabulary derived: {} f32 newtypes, {} type aliases",
        vocab.scalars.len(),
        vocab.aliases.len()
    );

    // Every source, not just usvg: fontdb defines enums too -- `Style` is
    // Normal/Italic/Oblique -- and a usvg-only pass left them to the object
    // path, which reported them as "not a public struct" and dropped the field
    // that used them. Driven by the list so a third crate is a push.
    let mut enums = TokenStream::new();
    let mut enum_names: BTreeSet<String> = BTreeSet::new();
    for src in &sources {
        let (names, code, rejected) = map_enums(&src.files, &referenced, &modules, &src.root);
        enum_names.extend(names);
        enums.extend(code);
        for (name, why) in rejected {
            report!("payload enum {name} not mapped: {why}");
        }
    }
    // `enum_names` already carries every source's enums, fontdb's included.
    vocab.enums = enum_names.clone();
    for src in &sources {
        vocab.ints.extend(int_newtypes(&src.files));
    }

    // Fixpoint over object types: a generated object can reference another
    // (`Stop` holds a `Color`).
    let mut object_todo: std::collections::VecDeque<String> = object_seeds.into_iter().collect();
    let mut object_done: BTreeSet<String> = BTreeSet::new();
    let mut object_root: BTreeMap<String, TokenStream> = BTreeMap::new();
    // Types given a second turn once their dependencies were discovered.
    let mut retried: BTreeSet<String> = BTreeSet::new();
    // Phase 1: discover. Generation is only used here to learn what a type
    // reaches; the code is thrown away because the registry is still growing.
    while let Some(t) = object_todo.pop_front() {
        if !object_done.insert(t.clone()) {
            continue;
        }
        // A runaway guard, not a budget: it used to be 64 and legitimate growth
        // reached it -- seventeen filter primitives and three light sources --
        // after which discovery stopped and the types it had not got to yet
        // were silently missing. Loud now, because a truncated registry looks
        // exactly like a type that does not map.
        assert!(
            object_done.len() <= 512,
            "object discovery passed 512 types at {t}: either the graph is cyclic \
             or this bound needs raising, but it must not truncate in silence"
        );
        if vocab.enums.contains(&t) || vocab.scalars.contains(&t) || vocab.ints.contains(&t) {
            continue;
        }
        let scoped = t.rsplit_once("::").and_then(|(m, _)| by_module.get(m));
        let (source, root) = if let Some(files) = scoped {
            (files, quote!(usvg))
        } else if let Some(src) = defines(&t) {
            (&src.files, src.root.clone())
        } else {
            report!("object {t} skipped: not a public struct");
            continue;
        };
        vocab.objects.insert(t.clone());
        object_root.insert(t.clone(), root.clone());
        let (members, dropped, reached) = data_members(source, &t, &vocab, true);
        // Seeding runs before the emptiness check, and a type that seeded
        // something gets one more turn.
        //
        // A type whose every member names a type not discovered yet maps to
        // nothing *yet*, and judging it here made that verdict permanent: the
        // dependencies it would have queued never were. `TextDecoration`
        // reaches only `TextDecorationStyle`, so it was reported as having
        // nothing mappable and the pair stayed invisible -- underline, overline
        // and line-through with them.
        let mut seeded = false;
        // A dropped member whose type is itself a public struct is a candidate:
        // that is how `Stop` reaches `Color`.
        for d in &dropped {
            let bare = peel_type(d.ty.as_deref().unwrap_or_default())
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .to_string();
            if !bare.is_empty() && !object_done.contains(&bare) {
                object_todo.push_back(bare);
                seeded = true;
            }
        }
        let fresh: Vec<_> = reached
            .into_iter()
            .filter(|x| !object_done.contains(x))
            .collect();
        seeded |= !fresh.is_empty();
        object_todo.extend(fresh);
        if members.is_empty() {
            vocab.objects.remove(&t);
            object_root.remove(&t);
            // One retry, and only when this turn queued something new: without
            // both conditions a cycle of mutually undiscovered types would keep
            // re-queueing each other forever.
            if seeded && retried.insert(t.clone()) {
                object_done.remove(&t);
                object_todo.push_back(t);
                continue;
            }
            report!("data type {t} skipped: nothing mappable on it");
            continue;
        }
    }
    // Phase 2, with the registry complete: strict mapping becomes an object,
    // partial mapping becomes a read-only class. Deciding this in phase 1 got it
    // wrong, because a nested type may not have been discovered yet.
    //
    // Settled to a fixpoint *before* anything is emitted. A read-only class
    // cannot be a field of an object -- napi needs Clone and FromNapiValue,
    // which a class has neither of -- so demoting one type forces every type
    // holding it down as well. Deciding and emitting in one pass got that
    // wrong in registry order: `Path` sorts before `Stroke`, so Path was built
    // holding `Option<Stroke>` as an object field, and Stroke was demoted a
    // moment later. The cascade itself is automatic: `classify` maps a demoted
    // type to `Js::Value`, `data_members` has no field mapping for one, so it
    // lands in `skipped`, and `object_struct` refuses any type with a skipped
    // member.
    let source_of = |t: &str| -> &[syn::File] {
        if let Some((m, _)) = t.rsplit_once("::")
            && let Some(files) = by_module.get(m)
        {
            return files;
        }
        // The last source is the fallback rather than an error: a type that
        // reaches here is one the registry already accepted, so the question is
        // which crate to read it from, not whether it exists.
        defines(t).map(|s| s.files.as_slice()).unwrap_or_else(|| {
            sources
                .last()
                .expect("at least one source")
                .files
                .as_slice()
        })
    };
    loop {
        let demote: Vec<String> = vocab
            .objects
            .iter()
            .filter_map(|t| {
                let (code, _, _) =
                    object_struct(source_of(t), t, &vocab, &modules, &object_root[t]);
                code.is_empty().then(|| t.clone())
            })
            .collect();
        if demote.is_empty() {
            break;
        }
        for t in demote {
            vocab.objects.remove(&t);
            let (vcode, dropped, _) =
                value_class(source_of(&t), &t, &vocab, &modules, &object_root[&t]);
            if !vcode.is_empty() {
                vocab.values.insert(t);
            } else {
                // Neither an object nor a read-only class. It used to leave the
                // registry with nothing said, which reads exactly like a type
                // that was never a candidate.
                report!(
                    "{t} dropped: nothing maps, not even read-only ({})",
                    dropped
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }

    let mut object_parts: BTreeMap<String, TokenStream> = BTreeMap::new();
    for t in &vocab.objects {
        let (code, dropped, _) = object_struct(source_of(t), t, &vocab, &modules, &object_root[t]);
        // The invariant the fixpoint above exists to establish, checked rather
        // than trusted: anything still classed as an object maps in full. A
        // type left here with unmapped members would be emitted as
        // `#[napi(object)]` carrying a field napi cannot round-trip, and the
        // failure would land in the generated crate -- a confusing place to
        // read it. Decide-and-emit in one pass breaks exactly this.
        assert!(
            !code.is_empty(),
            "{t} is still an object candidate but maps only partially ({}). \
             The object/value fixpoint did not settle before emission.",
            dropped
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        object_parts.insert(t.clone(), code);
    }
    // (class, member, label, detail) for everything the generator left out,
    // reported once the template's own methods are known: a member it adds by
    // hand is covered, not missing.
    let mut unexposed: Vec<(String, String, String, String)> = Vec::new();
    for t in &vocab.values {
        // The dropped list is the *class's* own: a getter maps members an
        // object field cannot, so reporting the object probe's list here would
        // name members that are in fact exposed.
        let (vcode, dropped, _) = value_class(source_of(t), t, &vocab, &modules, &object_root[t]);
        object_parts.insert(t.clone(), vcode);
        report!(
            "{t}: partial mapping, emitted as read-only class {}",
            data_ident(t)
        );
        for d in dropped {
            unexposed.push((
                data_ident(t).to_string(),
                d.name.clone(),
                format!("{t} member"),
                d.to_string(),
            ));
        }
    }

    let (fields, skipped_fields, _) =
        map_struct(&primary_files, PARSE_OPTIONS, &vocab, precision_max);
    let (write_fields, skipped_write, clamped) =
        map_struct(&primary_files, WRITE_OPTIONS, &vocab, precision_max);
    // The write options are where the clamped indices live (config::CLAMP),
    // found by a name suffix. A rename upstream would match nothing, fall
    // through to u8::MAX, and hand usvg an index 242 places past the end of a
    // 13-entry table -- an out-of-bounds panic inside an `extern "C"` callback,
    // which aborts the process rather than throwing. Loud here instead.
    assert!(
        clamped > 0,
        "no {WRITE_OPTIONS} field matched `*{}`: either upstream renamed them, \
         in which case this clamp now protects nothing, or they are gone",
        CLAMP.0
    );

    // Fixpoint: start from the handle types the wrapped impls return, generate a
    // class for each, then follow the handles *those* classes return. The type
    // graph is finite; the cap is only a runaway guard.
    let handles_of = |set: &BTreeSet<String>| -> BTreeSet<String> {
        set.iter()
            .filter_map(|t| match vocab.classify(t) {
                Some(Js::Handle(x)) | Some(Js::HandleList(x)) => Some(x),
                _ => None,
            })
            .collect()
    };
    let mut todo: Vec<String> = handles_of(&referenced).into_iter().collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut wrapper_code = TokenStream::new();
    while let Some(t) = todo.pop() {
        if !done.insert(t.clone()) {
            continue;
        }
        // A runaway guard, not a budget -- the same rule the object loop was
        // given. It used to be a silent `|| done.len() > 24`, which dropped
        // whatever the LIFO pop order reached last while `report!` still listed
        // it among the classes generated: the report named types the build had
        // never emitted, and the build then failed on them.
        assert!(
            done.len() <= 512,
            "handle discovery passed 512 types at {t}: either the graph is cyclic \
             or this bound needs raising, but it must not truncate in silence"
        );
        let name = wrapper_ident(&t).to_string();
        if RESERVED.contains(&name.as_str()) {
            report!("handle {t} skipped: name {name} is taken");
            continue;
        }
        // `fontdb::Database` maps onto the hand-written FontDatabase class.
        if HAND_WRAPPED.iter().any(|(h, _)| *h == t) {
            continue;
        }
        let src = defines(&t).unwrap_or(&sources[0]);
        let (code, skipped, reached) = wrapper_class(&src.files, &t, &vocab, &modules, &src.root);
        if code.is_empty() {
            report!("handle {t} skipped: nothing mappable on it");
        } else {
            wrapper_code.extend(code);
        }
        for s in skipped {
            // `root()` is served by the generated `children()` view above.
            if s.name == "root" {
                report!("{}::{t} method covered by the content view: {s}", src.label);
            } else {
                unexposed.push((
                    name.clone(),
                    s.name.clone(),
                    format!("{}::{t} method", src.label),
                    s.to_string(),
                ));
            }
        }
        todo.extend(reached.into_iter().filter(|x| !done.contains(x)));
    }
    report!(
        "wrapper classes generated: {}",
        done.iter().cloned().collect::<Vec<_>>().join(", ")
    );

    // One declarative table (config::PASSES) instead of four call sites:
    // adding a wrapped `impl` block is a single row.
    struct Pass<'a> {
        label: &'a str,
        target: &'a str,
        files: &'a [syn::File],
        ty: &'a str,
        receiver: TokenStream,
        skip: &'a [&'a str],
        prologue: Option<TokenStream>,
        optional: bool,
    }
    let passes: Vec<Pass> = PASSES
        .iter()
        .map(|p| Pass {
            label: p.label,
            target: p.target,
            files: source_files(p.source),
            ty: p.ty,
            receiver: p
                .receiver
                .parse()
                .expect("a config::PASSES receiver that parses"),
            skip: p.skip,
            prologue: p
                .prologue
                .map(|t| t.parse().expect("a config::PASSES prologue that parses")),
            optional: p.optional,
        })
        .collect();

    let mut generated: BTreeMap<&str, TokenStream> = BTreeMap::new();
    let mut taken: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    // Reported once the template exists: whether a skipped method is "covered"
    // is a question about the template, which is built further down.
    let mut skips: Vec<(&str, &str, Vec<Skip>)> = Vec::new();
    for p in &passes {
        let names = taken.entry(p.target).or_default();
        let (code, skipped) = map_methods(
            &MethodPass {
                files: p.files,
                ty: p.ty,
                receiver: &p.receiver,
                skip: p.skip,
                prologue: p.prologue.as_ref(),
                readonly: false,
                optional: p.optional,
            },
            &vocab,
            names,
        );
        skips.push((p.label, p.target, skipped));
        generated.insert(p.label, code);
    }
    for (pass, what, by) in COVERED_ELSEWHERE {
        report!("{pass} method covered by {by}: {what}");
    }
    // Prune: an object type nothing returns is dead TS surface. Start from the
    // generated method bodies, then follow objects nested in kept objects.
    let mentions = |code: &TokenStream, name: &str| -> bool {
        format!(" {code} ").contains(&format!(" {name} "))
    };
    // The template hands out types as well -- `SvgNode::path` returns `Path` --
    // and none of that is visible from the passes. Probe it once, empty.
    // A union references its payload types, and unions are generated after this
    // pruning -- so without this the primitives `filter::Kind` names are dropped
    // as unreachable and the union cannot compile. Only for unions something
    // actually calls.
    let payload_map = vocab.payload.clone();
    let via_payload: BTreeSet<String> = payload_map
        .iter()
        .filter(|(_, p)| p.payload_blocker(&vocab).is_none())
        .filter(|(n, p)| {
            let c = p.conv_ident(n).to_string();
            let used = |t: &TokenStream| calls(t, &c);
            generated.values().any(used)
                || object_parts.values().any(used)
                || used(&wrapper_code)
                || used(&probe)
        })
        .flat_map(|(_, p)| p.variants.iter().map(|(_, x)| x.clone()))
        .flat_map(|payload| match payload {
            Payload::None => Vec::new(),
            Payload::Value(t) => vec![t],
            Payload::Fields(f) => f.into_iter().map(|(_, t)| t).collect(),
        })
        .map(|t| bare(&t).to_string())
        .collect();

    let mut kept: BTreeSet<String> = BTreeSet::new();
    let all_data: BTreeSet<String> = vocab.objects.union(&vocab.values).cloned().collect();
    for name in all_data.clone() {
        // A value class is referenced under its JS name, not the Rust one.
        let js = data_ident(&name).to_string();
        let used = generated
            .values()
            .any(|c| mentions(c, &name) || mentions(c, &js))
            || mentions(&wrapper_code, &name)
            || mentions(&wrapper_code, &js)
            || mentions(&probe, &name)
            || mentions(&probe, &js)
            || via_payload.contains(bare(&name));
        if used {
            kept.insert(name);
        }
    }
    loop {
        let mut added = false;
        for name in all_data.clone() {
            if kept.contains(&name) {
                continue;
            }
            if kept
                .iter()
                .any(|k| object_parts.get(k).is_some_and(|c| mentions(c, &name)))
            {
                kept.insert(name);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let dropped: Vec<String> = all_data.difference(&kept).cloned().collect();
    if !dropped.is_empty() {
        report!("object types pruned as unreachable: {}", dropped.join(", "));
    }
    let mut object_code: TokenStream = kept
        .iter()
        .filter_map(|k| object_parts.get(k).cloned())
        .collect();

    // The unions for the payload enums something actually reached. Generated
    // last, because a payload may be one of the data types settled just above,
    // and appended only when the converter is referenced -- an unreferenced
    // union is dead TS surface, the same reason the pruner exists.
    let mut unions: Vec<String> = Vec::new();
    for (name, info) in &vocab.payload {
        if let Some(why) = info.payload_blocker(&vocab) {
            report!("payload enum {name} not mapped: {why}");
            continue;
        }
        // `calls`, not the space-padded `mentions` used for type names: a
        // token stream renders a call as `foo_to_js(..)` with no space before
        // the paren.
        let conv = info.conv_ident(name).to_string();
        let used = |c: &TokenStream| calls(c, &conv);
        if !used(&object_code) && !generated.values().any(used) {
            continue;
        }
        match payload_enum_code(name, info, &vocab, &modules) {
            Ok(code) => {
                object_code.extend(code);
                unions.push(name.clone());
            }
            Err(why) => report!("payload enum {name} skipped: {why}"),
        }
    }
    if !unions.is_empty() {
        report!("payload enums emitted as unions: {}", unions.join(", "));
    }
    report!(
        "data types emitted: {}",
        kept.iter().cloned().collect::<Vec<_>>().join(", ")
    );

    let fontdb_methods = &generated["fontdb::Database"];
    let tree_methods = &generated["usvg::Tree"];
    let group_methods = &generated["usvg::Group"];
    let node_methods = &{
        let node = &generated["usvg::Node"];
        let group = &generated["usvg::Group (on SvgNode)"];
        quote!(#node #group)
    };

    for s in skipped_fields {
        report!("{}::{PARSE_OPTIONS} field not exposed: {s}", primary.label);
    }
    for s in skipped_write {
        report!("{}::{WRITE_OPTIONS} field not exposed: {s}", primary.label);
    }

    let decls: Vec<_> = fields.iter().map(|f| &f.decl).collect();
    let assigns: Vec<_> = fields.iter().map(|f| &f.assign).collect();
    let write_decls: Vec<_> = write_fields.iter().map(|f| &f.decl).collect();
    let write_assigns: Vec<_> = write_fields.iter().map(|f| &f.assign).collect();

    // Everything the passes above produced, in one value: `template` is called
    // twice and only the method lists differ between the two.
    let fragments = Fragments {
        enums,
        object_code,
        wrapper_code,
        decls,
        assigns,
        write_decls,
        write_assigns,
    };
    // What the template covers is derived from the template: build it once with
    // no generated methods, and read its own method names off it.
    let empty = TokenStream::new();
    let hand = template_fns(&template(&fragments, &empty, &empty, &empty, &empty));
    let defined: BTreeSet<&String> = hand.values().flatten().collect();
    // Members the generator left out of a class, unless the template adds
    // them to that same class by hand.
    for (class, member, label, s) in &unexposed {
        let verb = if hand.get(class).is_some_and(|fns| fns.contains(member)) {
            "covered by the template"
        } else {
            "not exposed"
        };
        report!("{label} {verb}: {s}");
    }
    for (from, to) in RENAMED {
        assert!(
            defined.iter().any(|n| n.as_str() == *to),
            "RENAMED says `{from}` is covered by `{to}`, but the template defines no `{to}`"
        );
    }
    for (label, target, skipped) in &skips {
        for s in skipped {
            let name = s.name.as_str();
            let renamed = RENAMED.iter().find(|(from, _)| *from == name);
            let verb = match renamed {
                Some((_, to)) => format!("covered by the template as `{to}`"),
                None if defined.iter().any(|n| n.as_str() == name) => {
                    "covered by the template, not generated".to_string()
                }
                // Another pass emitted a method of that name on the same class.
                None if taken.get(target).is_some_and(|t| t.contains(name)) => {
                    "covered by another pass on the same class".to_string()
                }
                None => "not exposed".to_string(),
            };
            report!("{label} method {verb}: {s}");
        }
    }

    let mut code = template(
        &fragments,
        fontdb_methods,
        tree_methods,
        group_methods,
        node_methods,
    );

    // Async twins: the template marks a Send-safe core, the rule writes the
    // ceremony. The marker is stripped on the way out -- rustc has never heard
    // of it.
    {
        let mut file: syn::File =
            syn::parse2(code.clone()).expect("the emitter template is not valid Rust");
        let twins = async_twins(&mut file);
        for t in &twins {
            report!(
                "async twin generated: {}::{} -> {} (Promise<{}>)",
                ty_str(&t.target),
                t.core,
                t.public,
                t.js
            );
        }
        let ceremony = emit_twins(&twins);
        code = quote! { #file #ceremony };
    }

    // Nothing above prevents a future upstream method from colliding with a
    // name the template already uses: rustc would say so, from inside a
    // generated file. Say it here instead.
    for (target, names) in template_fns(&code) {
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        for n in &names {
            assert!(
                seen.insert(n),
                "{target}::{n} is emitted twice: the template defines it and a \
                 pass generated it too. Add it to RENAMED, or skip it in the pass."
            );
        }
    }

    let header = format!(
        "// @generated by build.rs from {GENERATED_FROM}. DO NOT EDIT.\n{}",
        HEADER_REST
    );
    const HEADER_REST: &str = concat!(
        "// Regenerate with `touch build.rs && cargo build`.\n",
        "#![allow(dead_code)]\n",
        // Generated by type text, not type checking: an upstream alias may
        // already be the target type, option structs are filled field by
        // field from Default, and a 17-variant Either is what napi offers for
        // a JS union; `FontDatabase::new` is a JS constructor, so a Default
        // impl would be dead API. Everything else clippy says applies here too.
        "#![allow(clippy::useless_conversion, clippy::field_reassign_with_default, clippy::type_complexity, clippy::new_without_default)]\n",
    );
    let text = format!("{header}{}", code);

    // Staging copy, handy for inspection: $OUT_DIR/bindings.rs
    let staged = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    fs::write(&staged, &text).unwrap();
    rustfmt(&staged);

    // Real target. It has to be the crate root itself: `napi-derive` keeps a
    // global struct registry and requires `#[napi] struct` to expand before
    // `#[napi] impl`, which `include!(...)` does not preserve.
    let dest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("src/lib.rs");
    let formatted = fs::read_to_string(&staged).unwrap_or(text);
    // Write only on change, otherwise the touched mtime re-triggers the build.
    if fs::read_to_string(&dest).ok().as_deref() != Some(formatted.as_str()) {
        fs::write(&dest, &formatted).unwrap();
    }
}

fn rustfmt(path: &Path) {
    // Best effort: readable output when rustfmt is around, ignored otherwise.
    let _ = std::process::Command::new("rustfmt")
        .arg("--edition=2024")
        .arg(path)
        .status();
}

/// Whether `ident` occurs in `code` as a whole identifier. A substring test
/// finds `kind_to_js` inside `image_kind_to_js` and `color_matrix_kind_to_js`,
/// which would keep `filter::Kind`'s union alive with nothing calling it.
fn calls(code: &TokenStream, ident: &str) -> bool {
    code.to_string()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w == ident)
}
