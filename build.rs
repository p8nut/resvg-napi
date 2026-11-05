//! Generates the whole NAPI binding layer from the *actual* resvg / usvg /
//! fontdb sources: `syn` parses them, `quote` re-emits Rust.
//!
//! The generic phases live in their own crate, `crates/napi-gen` (sources,
//! vocab, emit, cfg); this one fills its config and orchestrates them:
//!
//!   build/config.rs    the resvg-specific lists: renames, reserved names,
//!                      hand-wired fields, and the `napi_gen::Config` they
//!                      make. What another crate would rewrite
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
use syn::Item;

/// The env var that turns the codegen's report (`napi_gen::report!`) on.
const CODEGEN_LOG: &str = "RESVG_NAPI_CODEGEN_LOG";

// The resvg-specific files. `#[path]` because Cargo compiles this one as a
// crate root, so a plain `mod` would look for the file beside it -- in the
// repository root.
#[path = "build/config.rs"]
mod config;
#[path = "build/template.rs"]
mod template;

use config::*;
use napi_gen::emit::*;
use napi_gen::report;
use napi_gen::sources::*;
use napi_gen::vocab::*;
use template::*;

fn main() {
    napi_build::setup();
    napi_gen::configure(GEN);
    // One table: a kind that both a getter and a method can return has the
    // same JS type at both, whatever expression each uses to get there.
    {
        let (v, a) = (Vocab::default(), quote!(x));
        for js in [
            Js::Bool,
            Js::I32,
            Js::F32,
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
    println!("cargo::rerun-if-changed=build.rs");
    // Cargo watches only what it is told: without these, editing config or the
    // template would leave src/lib.rs stale and the CI drift check would fail
    // on someone else's machine rather than here. napi-gen, a path dependency,
    // Cargo tracks on its own.
    for m in ["config", "template"] {
        println!("cargo::rerun-if-changed=build/{m}.rs");
    }
    // The generated file is also an *input* to watch: if someone deletes or
    // stubs src/lib.rs, the script must re-run and regenerate it.
    println!("cargo::rerun-if-changed=src/lib.rs");
    println!("cargo::rerun-if-env-changed={CODEGEN_LOG}");

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
    // `root_str` is kept beside `root`: quote! spells a path with spaces
    // around `::`, and this one reaches the report a reader is expected to
    // grep.
    struct Source {
        files: Vec<syn::File>,
        parsed: Vec<(PathBuf, syn::File)>,
        tree: ModTree,
        root: TokenStream,
        root_str: &'static str,
    }
    let sources: Vec<Source> = UPSTREAM
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let dir = locate(u.pkg, u.marker, &locked);
            if i == 0 {
                report!("{} sources: {}", u.root, dir.display());
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
            }
        })
        .collect();
    // The primary crate: options and clamps are read from it.
    let primary = &sources[0];
    let primary_files = &primary.files;
    let modules = &primary.tree.paths;
    // Its files grouped by the namespace they export into. A bare-name lookup
    // over the whole crate takes whichever file was parsed first, which is how
    // `filter::Image` came back with `usvg::Image`'s methods; a qualified key
    // resolves against its own module instead.
    let mut by_module: BTreeMap<String, Vec<syn::File>> = BTreeMap::new();
    for (path, file) in &primary.parsed {
        let m = primary.tree.file_ns.get(path).cloned().unwrap_or_default();
        by_module.entry(m).or_default().push(file.clone());
    }
    let source_files = |root: &str| {
        &sources
            .iter()
            .find(|s| s.root_str == root)
            .unwrap_or_else(|| panic!("no config::UPSTREAM with root {root:?}"))
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
            for id in f.items.iter().filter_map(item_ident) {
                *seen.entry(id.to_string()).or_default() += 1;
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

    // Each index-bounded field's highest usable index, read off its table.
    let bounds = index_bounds(primary_files);
    for ((suffix, table), (_, max)) in GEN.index_bounded.iter().zip(&bounds) {
        report!(
            "{} index bound for `*{suffix}` derived from {table}: {max}",
            primary.root_str
        );
    }

    // Enums to mirror = named by a config field OR by a return of a wrapped impl.
    let mut referenced = struct_field_types(primary_files, PARSE_OPTIONS);
    referenced.extend(struct_field_types(primary_files, WRITE_OPTIONS));
    referenced.extend(returned_types(primary_files));
    for src in &sources[1..] {
        referenced.extend(returned_types(&src.files));
        referenced.extend(field_types(&src.files));
    }

    // Object candidates: a *collection* of values wants a value type, whereas a
    // single borrow wants a handle. So `&[Stop]` and `impl Iterator<Item=&FaceInfo>`
    // make Stop/FaceInfo objects, while `&Group` stays a handle question.
    let object_seeds: BTreeSet<String> = referenced
        .iter()
        .filter(|t| {
            let b = t.trim_start_matches('&');
            b.starts_with('[') || b.starts_with("implIterator<Item=")
        })
        .map(|t| peel_type(t).to_string())
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
                .filter(|t| !GEN.node_types.contains(&bare(t)))
                .filter(|t| struct_fields_opt(primary_files, bare(t)).is_some()),
        )
        // Plus the projected structs (`GEN.projections`): a node payload is
        // not a collection, so nothing else nominates `Text`, and its chunks
        // and spans are data of the projection class. The projection is
        // `napi_gen::project`'s; its data type only reaches what it holds.
        .chain(
            GEN.projections
                .iter()
                .map(|(_, t)| t.to_string())
                .filter(|t| {
                    // Not one already claimed as a handle: an `Arc`-held
                    // definition (`ClipPath`) would get `wrap` twice.
                    let arc_held = referenced.iter().any(|r| {
                        peel_type(r)
                            .strip_prefix("Arc<")
                            .and_then(|i| i.strip_suffix('>'))
                            == Some(t.as_str())
                    });
                    !arc_held
                        && !GEN.node_types.contains(&t.as_str())
                        && sources
                            .iter()
                            .any(|s| struct_fields_opt(&s.files, t).is_some())
                }),
        )
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
    // The second surface, by the root of the source that defines each item:
    // `napi_gen::emit::namespaced` copies it under that source's namespace.
    let mut mirrored: BTreeMap<&str, TokenStream> = BTreeMap::new();
    for src in &sources {
        let (names, code, rejected) = map_enums(&src.files, &referenced, modules, &src.root);
        enum_names.extend(names);
        mirrored
            .entry(src.root_str)
            .or_default()
            .extend(code.clone());
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
    // member. The last iteration, the one that demotes nothing, has built
    // every object's code already: that is what is emitted.
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
    let mut object_parts: BTreeMap<String, TokenStream> = loop {
        let mut parts = BTreeMap::new();
        let mut demote = Vec::new();
        for t in &vocab.objects {
            let (code, _, _) = object_struct(source_of(t), t, &vocab, modules, &object_root[t]);
            if code.is_empty() {
                demote.push(t.clone());
            } else {
                parts.insert(t.clone(), code);
            }
        }
        if demote.is_empty() {
            break parts;
        }
        for t in demote {
            vocab.objects.remove(&t);
            let (vcode, dropped, _) =
                value_class(source_of(&t), &t, &vocab, modules, &object_root[&t]);
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
    };
    // (class, member, label, detail) for everything the generator left out,
    // reported once the template's own methods are known: a member it adds by
    // hand is covered, not missing.
    let mut unexposed: Vec<(String, String, String, String)> = Vec::new();
    for t in &vocab.values {
        // The dropped list is the *class's* own: a getter maps members an
        // object field cannot, so reporting the object probe's list here would
        // name members that are in fact exposed.
        let (vcode, dropped, _) = value_class(source_of(t), t, &vocab, modules, &object_root[t]);
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

    let (fields, skipped_fields, _) = map_struct(primary_files, PARSE_OPTIONS, &vocab, &bounds);
    let (write_fields, skipped_write, clamped) =
        map_struct(primary_files, WRITE_OPTIONS, &vocab, &bounds);
    // The write options are where the clamped indices live
    // (config::GEN.index_bounded), found by a name suffix. A rename upstream
    // would match nothing, fall through to u8::MAX, and hand usvg an index 242
    // places past the end of a 13-entry table -- an out-of-bounds panic inside
    // an `extern "C"` callback, which aborts the process rather than throwing.
    // Loud here instead.
    assert!(
        clamped > 0,
        "no {WRITE_OPTIONS} field matched an index bound: either upstream renamed \
         them, in which case the clamp now protects nothing, or they are gone"
    );

    // The option structs as plain objects of the namespaced surface
    // (`usvg.Options`, `usvg.WriteOptions`): every field optional over
    // `Default`, the font database supplied by a `fontdb.Database`.
    for name in GEN.option_objects {
        let Some(obj) = option_object(primary_files, name, &vocab, modules, &primary.root, &bounds)
        else {
            report!(
                "{}::{name} not an option object: no Default to fill it from",
                primary.root_str
            );
            continue;
        };
        // The same guard as above, for the namespaced copy.
        assert!(
            *name != WRITE_OPTIONS || obj.bounded == clamped,
            "{name}: the option object clamps {} fields, the flat one {clamped}",
            obj.bounded
        );
        for s in obj.skipped {
            report!(
                "{}::{name} field not in the option object: {s}",
                primary.root_str
            );
        }
        mirrored
            .entry(primary.root_str)
            .or_default()
            .extend(obj.code);
    }

    // Fixpoint: start from the handle types the wrapped impls return, generate a
    // class for each, then follow the handles *those* classes return. The type
    // graph is finite, and `done` keeps a type from being visited twice.
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
    while let Some(t) = todo.pop() {
        if !done.insert(t.clone()) {
            continue;
        }
        // A key spelled through the crate that re-exports the type
        // (`<crate>::Type`) is taken relative to the defining one.
        let src = defines(&t).unwrap_or(&sources[0]);
        let key = src
            .root_str
            .rsplit("::")
            .next()
            .and_then(|last| t.strip_prefix(&format!("{last}::")))
            .filter(|_| src.root_str.contains("::"))
            .unwrap_or(&t);
        // A class `GEN.classes` lists is `napi_gen::class`'s, below.
        // So is a projection (`GEN.projections`), by its full key: a
        // module's type of the same name is another one.
        let listed = GEN
            .classes
            .iter()
            .any(|(r, n)| *r == src.root_str && *n == bare(key))
            || GEN
                .projections
                .iter()
                .any(|(r, n)| *r == src.root_str && *n == key);
        if listed {
            continue;
        }
        let (code, skipped, reached) = wrapper_class(&src.files, key, &vocab, modules, &src.root);
        for s in skipped {
            report!(
                "{}::{key} method not in the namespaced class: {s}",
                src.root_str
            );
        }
        mirrored.entry(src.root_str).or_default().extend(code);
        todo.extend(reached.into_iter().filter(|x| !done.contains(x)));
    }
    report!(
        "wrapper classes generated: {}",
        done.iter().cloned().collect::<Vec<_>>().join(", ")
    );

    // The classes and free functions of the second surface (`GEN.classes`,
    // `GEN.functions`): over the wrapped crates, and over the crates parsed
    // for them alone (`CLASS_CRATES`), each read as one source. The crate
    // root's files are where its free functions are.
    struct ClassCrate {
        files: Vec<syn::File>,
        lib: Vec<syn::File>,
        modules: BTreeMap<String, String>,
        root: &'static str,
    }
    let extra: Vec<ClassCrate> = CLASS_CRATES
        .iter()
        .map(|(pkgs, root)| {
            let (mut files, mut lib, mut modules) = (Vec::new(), Vec::new(), BTreeMap::new());
            for pkg in *pkgs {
                let dir = locate(pkg, "lib.rs", &locked);
                let all = parse_crate(&dir);
                let tree = ModTree::load(&dir, &all);
                for (path, file) in tree.public_view(&all, &dir) {
                    if path == dir.join("lib.rs") && lib.is_empty() {
                        lib.push(file.clone());
                    }
                    files.push(file);
                }
                for (name, at) in tree.paths {
                    modules.entry(name).or_insert(at);
                }
            }
            ClassCrate {
                files,
                lib,
                modules,
                root,
            }
        })
        .collect();
    // A wrapped crate's classes are its root module's: a module whose types
    // are exported under its own path (`filter`) is not that source, and its
    // `Image` is not `usvg::Image`.
    let root_files: Vec<Vec<syn::File>> = sources
        .iter()
        .map(|s| {
            s.parsed
                .iter()
                .filter(|(p, _)| s.tree.file_ns.get(p).is_none_or(String::is_empty))
                .map(|(_, f)| f.clone())
                .collect()
        })
        .collect();
    let classes = napi_gen::class::Classes::new(
        sources
            .iter()
            .zip(&root_files)
            .map(|(s, files)| napi_gen::class::Source {
                root: s.root_str,
                files,
                modules: &s.tree.paths,
            })
            .chain(extra.iter().map(|c| napi_gen::class::Source {
                root: c.root,
                files: &c.files,
                modules: &c.modules,
            }))
            .collect(),
    );
    for (i, c) in classes.classes.iter().enumerate() {
        let root = classes.sources[c.source].root;
        let (code, skipped) = classes.class(i, &vocab);
        let mut what = format!("{:?}", c.holding);
        if !c.plain.is_empty() {
            what.push_str(", plain data");
        }
        if c.shared_name {
            what.push_str(", a JS name another namespace has");
        }
        report!("{root}::{} is a class: {what}", c.name);
        for s in skipped {
            report!("{root}::{} member not in the class: {s}", c.name);
        }
        mirrored.entry(root).or_default().extend(code);
    }
    for ClassCrate { lib, root, .. } in &extra {
        let (code, skipped) = classes.functions(root, lib, &vocab);
        for s in skipped {
            report!("{root} function not exposed: {s}");
        }
        mirrored.entry(root).or_default().extend(code);
    }
    for u in &classes.unions {
        let root = classes.sources[u.source].root;
        report!("{root}::{} is a union of projections", u.name);
    }
    for (s, code) in classes
        .error_codes()
        .into_iter()
        .chain(classes.union_code())
    {
        mirrored
            .entry(classes.sources[s].root)
            .or_default()
            .extend(code);
    }

    // One declarative table (config::PASSES) instead of four call sites:
    // adding a wrapped `impl` block is a single row.
    let mut generated: BTreeMap<&str, TokenStream> = BTreeMap::new();
    let mut taken: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    // Reported once the template exists: whether a skipped method is "covered"
    // is a question about the template, which is built further down.
    let mut skips: Vec<(&str, &str, Vec<Skip>)> = Vec::new();
    for p in PASSES {
        let names = taken.entry(p.target).or_default();
        let receiver: TokenStream = p
            .receiver
            .parse()
            .expect("a config::PASSES receiver that parses");
        let (code, skipped) = map_methods(
            &MethodPass {
                files: source_files(p.source),
                ty: p.ty,
                receiver: &receiver,
                receiver_mut: None,
                skip: p.skip,
                readonly: false,
                standin: true,
            },
            &vocab,
            names,
        );
        skips.push((p.label, p.target, skipped));
        generated.insert(p.label, code);
    }
    // Every settled data type, objects and read-only classes alike: the
    // registry only ever holds what a pass, the template or another data type
    // reached, so there is nothing to prune.
    let mut object_code: TokenStream = object_parts.values().cloned().collect();
    for (t, code) in &object_parts {
        let root = object_root[t].to_string().replace(' ', "");
        let src = sources
            .iter()
            .find(|s| s.root_str == root)
            .expect("a data type's root is a source's");
        // A type `GEN.projections` lists is `napi_gen::project`'s class.
        if GEN
            .projections
            .iter()
            .any(|(r, n)| *r == src.root_str && *n == t.as_str())
        {
            report!("{root}::{t} is a projection, not a copy of the flat data type");
            continue;
        }
        mirrored
            .entry(src.root_str)
            .or_default()
            .extend(code.clone());
    }

    // The unions for the payload enums. Generated last, because a payload may
    // be one of the data types settled just above.
    let mut unions: Vec<String> = Vec::new();
    for (name, info) in &vocab.payload {
        if let Some(why) = info.payload_blocker(&vocab) {
            report!("payload enum {name} not mapped: {why}");
            continue;
        }
        match payload_enum_code(name, info, &vocab, modules) {
            Ok(code) => {
                // In the namespace of the source defining the enum. One only
                // mined for vocabulary is another crate's, re-exported: it
                // waits for that crate's own namespace.
                match sources.iter().find(|s| {
                    s.files.iter().flat_map(|f| &f.items).any(
                        |i| matches!(i, Item::Enum(e) if e.ident == bare(name) && is_pub(&e.vis)),
                    )
                }) {
                    Some(src) => mirrored
                        .entry(src.root_str)
                        .or_default()
                        .extend(code.clone()),
                    None => report!("payload enum {name} not namespaced: no source defines it"),
                }
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
        object_parts.keys().cloned().collect::<Vec<_>>().join(", ")
    );

    let fontdb_methods = &generated["fontdb::Database"];
    let tree_methods = &generated["usvg::Tree"];
    let group_methods = &generated["usvg::Group"];

    for s in skipped_fields {
        report!(
            "{}::{PARSE_OPTIONS} field not exposed: {s}",
            primary.root_str
        );
    }
    for s in skipped_write {
        report!(
            "{}::{WRITE_OPTIONS} field not exposed: {s}",
            primary.root_str
        );
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
        decls,
        assigns,
        write_decls,
        write_assigns,
    };
    // What the template covers is derived from the template: build it once with
    // no generated methods, and read its own method names off it.
    let empty = TokenStream::new();
    let hand = template_fns(&template(&fragments, &empty, &empty, &empty));
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

    let mut code = template(&fragments, fontdb_methods, tree_methods, group_methods);
    // The second surface, one namespace per source (`GEN.namespaces`).
    {
        let parts: Vec<(&str, TokenStream)> = mirrored
            .into_iter()
            .map(|(root, code)| {
                let ns = napi_gen::namespace_of(root)
                    .unwrap_or_else(|| panic!("no GEN.namespaces entry for {root}"));
                (ns, code)
            })
            .collect();
        // The flat surface keeps only the generated data types it names, or
        // the namespaced copies do: the rest are reached through the
        // namespaces. napi keeps one constructor per JS class name, so a
        // copy with the name of a flat class is left out.
        let generated = [&fragments.enums, &fragments.object_code];
        let flat = prune(code.clone(), &generated, &[]);
        let surfaces = namespaced(&parts, &flat, &classes.shared_names());
        for line in surfaces.report {
            report!("{line}");
        }
        code = prune(code, &generated, &[&surfaces.namespaced]);
        code.extend(surfaces.namespaced);
    }

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
        let mut file: syn::File =
            syn::parse2(quote! { #file #ceremony }).expect("the async twins are valid Rust");
        guard_live(&mut file, "Resvg", &["dispose", "disposed"]);
        let inspect = inspect_members(&file);
        code = quote! { #file #inspect };
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

/// `code` without the items of `generated` that nothing else names: a
/// generated data type, enum or conversion stays while an item outside
/// `generated`, an item of `also`, or a kept one names it. Items are matched
/// by the name they declare (an impl by its type). A name after `::` or `.`
/// is a path or a member, and an enum's variant a variant: not references.
fn prune(code: TokenStream, generated: &[&TokenStream], also: &[&TokenStream]) -> TokenStream {
    fn name(item: &Item) -> Option<String> {
        match item {
            Item::Struct(s) => Some(s.ident.to_string()),
            Item::Enum(e) => Some(e.ident.to_string()),
            Item::Fn(f) => Some(f.sig.ident.to_string()),
            Item::Type(t) => Some(t.ident.to_string()),
            Item::Impl(i) => napi_gen::sources::self_ident(i).map(ToString::to_string),
            _ => None,
        }
    }
    fn names(ts: TokenStream, out: &mut BTreeSet<String>) {
        use proc_macro2::{Delimiter, TokenTree};
        let mut prev: Vec<char> = Vec::new();
        let mut enum_head = false;
        for t in ts {
            match &t {
                TokenTree::Ident(id) if id == "enum" => enum_head = true,
                TokenTree::Ident(id)
                    if !prev.ends_with(&[':', ':']) && prev.last() != Some(&'.') =>
                {
                    out.insert(id.to_string());
                }
                // A variant's own name is left out, what it carries is not.
                TokenTree::Group(g) if enum_head && g.delimiter() == Delimiter::Brace => {
                    enum_head = false;
                    for v in g.stream() {
                        if let TokenTree::Group(g) = v {
                            names(g.stream(), out);
                        }
                    }
                }
                TokenTree::Group(g) => names(g.stream(), out),
                _ => {}
            }
            match t {
                TokenTree::Punct(p) => prev.push(p.as_char()),
                _ => prev.clear(),
            }
        }
    }
    let parse = |ts: &TokenStream| -> syn::File {
        syn::parse2(ts.clone()).expect("generated code that parses")
    };
    let candidates: BTreeSet<String> = generated
        .iter()
        .flat_map(|ts| parse(ts).items)
        .filter_map(|i| name(&i))
        .collect();
    let file = parse(&code);
    let pending = |i: &Item| name(i).is_some_and(|n| candidates.contains(&n));
    let mut seen = BTreeSet::new();
    let mut keep: Vec<bool> = file.items.iter().map(|i| !pending(i)).collect();
    for (item, _) in file.items.iter().zip(&keep).filter(|(_, k)| **k) {
        names(quote!(#item), &mut seen);
    }
    for ts in also {
        names((*ts).clone(), &mut seen);
    }
    loop {
        let more: Vec<usize> = (0..file.items.len())
            .filter(|&i| !keep[i] && name(&file.items[i]).is_some_and(|n| seen.contains(&n)))
            .collect();
        if more.is_empty() {
            break;
        }
        for i in more {
            keep[i] = true;
            let item = &file.items[i];
            names(quote!(#item), &mut seen);
        }
    }
    let kept = file
        .items
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(i, _)| i);
    quote!(#(#kept)*)
}

fn rustfmt(path: &Path) {
    // Best effort: readable output when rustfmt is around, ignored otherwise.
    let _ = std::process::Command::new("rustfmt")
        .arg("--edition=2024")
        .arg(path)
        .status();
}
