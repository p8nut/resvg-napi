//! Generates the whole NAPI binding layer from the *actual* resvg / usvg /
//! fontdb sources: `syn` parses them, `quote` re-emits Rust.
//!
//! One phase per file, and this one orchestrates them:
//!
//!   build/sources.rs   finding and parsing upstream
//!                      (env override -> git submodule -> cargo registry)
//!   build/vocab.rs     what a Rust type means on the JS side -- newtypes,
//!                      aliases, `Deref` chains, payload enums, naming
//!   build/emit.rs      turning those answers into tokens napi can expand
//!   build/template.rs  the API decisions, written by hand. The only file here
//!                      that is not describing upstream
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
        if env::var_os("RESVG_NAPI_CODEGEN_LOG").is_some() {
            println!("cargo::warning={}", format!($($arg)*));
        }
    };
}

// Each phase in its own file. `#[path]` because Cargo compiles this one as a
// crate root, so a plain `mod` would look for the file beside it -- in the
// repository root.
#[path = "build/emit.rs"]
mod emit;
#[path = "build/sources.rs"]
mod sources;
#[path = "build/template.rs"]
mod template;
#[path = "build/vocab.rs"]
mod vocab;

use emit::*;
use sources::*;
use template::*;
use vocab::*;

// ---------------------------------------------------------------------------
// 1. source discovery
// ---------------------------------------------------------------------------

/// Upstream names the template exposes under a *different* one. A rule cannot
/// find these -- the rename is the decision -- so they are declared, and
/// checked: a stale entry fails the build instead of rotting in a log line.
const RENAMED: &[(&str, &str)] = &[
    ("from_data", "new"),
    ("from_str", "new"),
    ("from_xmltree", "new"),
    ("from_data_nested", "new"),
    ("node_by_id", "node"),
    // `to_string` in Rust would collide with `Display::to_string`
    ("to_string", "to_svg_string"),
    ("root", "children"),
    ("with_face_data", "face_data"),
];

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

fn assert_tree_ctor(files: &[syn::File], name: &str) {
    let found = files.iter().flat_map(|f| &f.items).any(|i| match i {
        // upstream writes `impl crate::Tree`, so compare the last path segment
        Item::Impl(imp) if ty_str(&imp.self_ty).rsplit("::").next() == Some("Tree") => imp
            .items
            .iter()
            .any(|it| matches!(it, ImplItem::Fn(f) if f.sig.ident == name && is_pub(&f.vis))),
        _ => false,
    });
    assert!(found, "usvg: `Tree::{name}` disappeared");
}

// ---------------------------------------------------------------------------
// 4. emission
// ---------------------------------------------------------------------------

fn main() {
    napi_build::setup();
    println!("cargo::rerun-if-changed=build.rs");
    // The generator is five files now, and Cargo watches only what it is told:
    // without these, editing a phase module would leave src/lib.rs stale and the
    // CI drift check would fail on someone else's machine rather than here.
    for m in ["sources", "vocab", "emit", "template"] {
        println!("cargo::rerun-if-changed=build/{m}.rs");
    }
    // The generated file is also an *input* to watch: if someone deletes or
    // stubs src/lib.rs, the script must re-run and regenerate it.
    println!("cargo::rerun-if-changed=src/lib.rs");
    println!("cargo::rerun-if-env-changed=RESVG_NAPI_EMIT_SRC");
    println!("cargo::rerun-if-env-changed=RESVG_NAPI_CODEGEN_LOG");

    let locked = locked_versions();
    let usvg = locate("usvg", "parser/options.rs", &locked);
    let resvg = locate("resvg", "lib.rs", &locked);
    let fontdb = locate("fontdb", "lib.rs", &locked);
    report!("usvg sources: {}", usvg.display());

    // Whole crates, no per-file list: the `impl` blocks we wrap are spread over
    // tree/mod.rs, tree/filter.rs, parser/, writer.rs...
    let usvg_parsed = parse_crate(&usvg);
    let usvg_files: Vec<syn::File> = usvg_parsed.iter().map(|(_, f)| f.clone()).collect();
    let public = public_modules(&usvg_files);
    let modules = upstream_modules(&usvg_parsed, &public);
    // usvg files grouped by the module they contribute to. A bare-name lookup
    // over the whole crate takes whichever file was parsed first, which is how
    // `filter::Image` came back with `usvg::Image`'s methods; a qualified key
    // resolves against its own module instead.
    let mut by_module: BTreeMap<String, Vec<syn::File>> = BTreeMap::new();
    for (path, file) in &usvg_parsed {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let m = if public.contains(&stem) {
            stem
        } else {
            String::new()
        };
        by_module.entry(m).or_default().push(file.clone());
    }
    // Public names usvg defines twice. Exactly one today, `Image`, and the
    // report says so rather than leaving it to be rediscovered.
    //
    // Enums count as well as structs, and they count together: `payload_enums`
    // qualifies a carried type by this set, so an enum defined in two modules --
    // or an enum in one and a struct of the same name in another -- would be
    // keyed by its bare name and resolve to whichever the walk reached first.
    // That is the failure `filter::Image` already caused once on the struct side.
    let dups: BTreeSet<String> = {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for f in &usvg_files {
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
            "names usvg defines twice, keyed by module: {}",
            dups.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    // Only the crate root for free functions: a module-level `pub fn` elsewhere
    // is not crate-public (resvg has an internal `render(&Image, ...)`).
    let resvg_root = vec![parse(&resvg.join("lib.rs"))];
    let fontdb_files: Vec<syn::File> = public_only(&fontdb, parse_crate(&fontdb))
        .into_iter()
        .map(|(_, f)| f)
        .collect();

    // Guards first: no point generating against a moved API.
    assert_free_fn(
        &resvg_root,
        "render",
        &[
            "&usvg::Tree",
            "tiny_skia::Transform",
            "&muttiny_skia::PixmapMut",
        ],
    );
    assert_tree_ctor(&usvg_files, "from_data");
    assert_tree_ctor(&usvg_files, "to_string");
    assert_struct_fields(
        &usvg_files,
        "ImageHrefResolver",
        &["resolve_data", "resolve_string"],
    );
    assert_struct_fields(
        &usvg_files,
        "FontResolver",
        &["select_font", "select_fallback"],
    );

    // POW_VEC has N entries, so the highest usable precision is N - 1.
    let precision_max = (static_array_len(&usvg_files, "POW_VEC") - 1) as u32;
    report!("usvg precision clamp derived from POW_VEC: {precision_max}");

    // Enums to mirror = named by a config field OR by a return of a wrapped impl.
    let mut referenced = struct_field_types(&usvg_files, "Options");
    referenced.extend(struct_field_types(&usvg_files, "WriteOptions"));
    referenced.extend(returned_types(&usvg_files, &[]));
    referenced.extend(returned_types(&fontdb_files, &[]));
    referenced.extend(field_types(&fontdb_files));

    // Object candidates: a *collection* of values wants a value type, whereas a
    // single borrow wants a handle. So `&[Stop]` and `impl Iterator<Item=&FaceInfo>`
    // make Stop/FaceInfo objects, while `&Group` stays a handle question.
    // Everything asked of the template is written in its own body, so the
    // fragments can be blank. Reused by the pruner further down.
    let nothing = TokenStream::new();
    let probe = template(&Fragments::probe(), &nothing, &nothing, &nothing, &nothing);
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
                .flat_map(|(m, files)| payload_enums(files, m, &dups))
                .flat_map(|(_, p)| p.variants)
                .flat_map(|(_, payload)| match payload {
                    Payload::None => Vec::new(),
                    Payload::Value(t) => vec![t],
                    Payload::Fields(f) => f.into_iter().map(|(_, t)| t).collect(),
                })
                .filter(|t| !NODE_TYPES.contains(&bare(t)))
                .filter(|t| struct_fields_opt(&usvg_files, bare(t)).is_some()),
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
                && (struct_fields_opt(&usvg_files, t).is_some()
                    || struct_fields_opt(&fontdb_files, t).is_some())
        }))
        .collect();

    let mut vocab = Vocab::default();
    let extra: Vec<syn::File> = ["tiny-skia-path", "strict-num"]
        .iter()
        .flat_map(|c| parse_crate(&locate(c, "lib.rs", &locked)))
        .map(|(_, f)| f)
        .collect();
    for set in [&usvg_files, &fontdb_files, &extra] {
        vocab.scalars.extend(f32_newtypes(set));
        vocab.aliases.extend(type_aliases(set));
        vocab.payload.extend(payload_enums(set, "", &dups));
        for item in set.iter().flat_map(|f| &f.items) {
            if let Item::Struct(st) = item {
                if is_pub(&st.vis) {
                    vocab.structs.insert(st.ident.to_string());
                }
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
        vocab.payload.extend(payload_enums(files, m, &dups));
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

    let (enum_names, enums) = map_enums(&usvg_files, &referenced, &modules, &quote!(usvg));
    // fontdb defines enums too -- `Style` is Normal/Italic/Oblique -- and a
    // usvg-only pass left them to the object path, which reported them as "not
    // a public struct" and dropped the field that used them.
    let (fontdb_enum_names, fontdb_enums) =
        map_enums(&fontdb_files, &referenced, &modules, &quote!(usvg::fontdb));
    let mut enums = enums;
    enums.extend(fontdb_enums);
    vocab.enums = enum_names.clone();
    vocab.enums.extend(fontdb_enum_names.iter().cloned());
    for set in [&usvg_files, &fontdb_files] {
        vocab.ints.extend(int_newtypes(set));
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
        } else if struct_fields_opt(&usvg_files, bare(&t)).is_some() {
            (&usvg_files, quote!(usvg))
        } else if struct_fields_opt(&fontdb_files, bare(&t)).is_some() {
            (&fontdb_files, quote!(usvg::fontdb))
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
            let raw = d.split(": ").nth(1).unwrap_or_default();
            let bare = raw
                .trim_start_matches('&')
                .trim_start_matches("Option<")
                .trim_start_matches("Vec<")
                .trim_end_matches('>')
                .trim_start_matches('&')
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
        if let Some((m, _)) = t.rsplit_once("::") {
            if let Some(files) = by_module.get(m) {
                return files;
            }
        }
        if struct_fields_opt(&usvg_files, bare(t)).is_some() {
            &usvg_files
        } else {
            &fontdb_files
        }
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
                    dropped.join(", ")
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
            dropped.join(", ")
        );
        object_parts.insert(t.clone(), code);
    }
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
            report!("{t} member not exposed: {d}");
        }
    }

    let (fields, skipped_fields, _) = map_struct(&usvg_files, "Options", &vocab, precision_max);
    let (write_fields, skipped_write, clamped) =
        map_struct(&usvg_files, "WriteOptions", &vocab, precision_max);
    // WriteOptions is where the POW_VEC indices live, and the fields are found
    // by a `_precision` suffix. A rename upstream would match nothing, fall
    // through to u8::MAX, and hand usvg an index 242 places past the end of a
    // 13-entry table -- an out-of-bounds panic inside an `extern "C"` callback,
    // which aborts the process rather than throwing. Loud here instead.
    assert!(
        clamped > 0,
        "no WriteOptions field matched `*_precision`: either usvg renamed them, \
         in which case this clamp now protects nothing, or they are gone"
    );

    // Fixpoint: start from the handle types the wrapped impls return, generate a
    // class for each, then follow the handles *those* classes return. The type
    // graph is finite; the cap is only a runaway guard.
    const RESERVED: [&str; 8] = [
        "Resvg",
        "SvgNode",
        "BBox",
        "Matrix",
        "Dimensions",
        "RawImage",
        "RenderOptions",
        "RenderParams",
    ];
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
        if t == "fontdb::Database" {
            continue;
        }
        let (code, skipped, reached) = wrapper_class(&usvg_files, &t, &vocab, &modules);
        if code.is_empty() {
            report!("handle {t} skipped: nothing mappable on it");
        } else {
            wrapper_code.extend(code);
        }
        for s in skipped {
            // `root()` is served by the generated `children()` view above.
            let verb = if s.starts_with("root ") {
                "covered by the content view"
            } else {
                "not exposed"
            };
            report!("usvg::{t} method {verb}: {s}");
        }
        todo.extend(reached.into_iter().filter(|x| !done.contains(x)));
    }
    report!(
        "wrapper classes generated: {}",
        done.iter().cloned().collect::<Vec<_>>().join(", ")
    );

    // One declarative table instead of four call sites: adding a wrapped
    // `impl` block is a single row.
    let node_prologue = quote!(let __n = self.node()?;);
    struct Pass<'a> {
        label: &'a str,
        target: &'a str,
        files: &'a [syn::File],
        ty: &'a str,
        receiver: TokenStream,
        skip: &'a [&'a str],
        prologue: Option<&'a TokenStream>,
    }
    let passes = [
        Pass {
            label: "fontdb::Database",
            target: "FontDatabase",
            files: &fontdb_files,
            ty: "Database",
            receiver: quote!(self.inner),
            skip: &[],
            prologue: None,
        },
        Pass {
            label: "usvg::Tree",
            target: "Resvg",
            files: &usvg_files,
            ty: "Tree",
            receiver: quote!(self.tree),
            skip: &[],
            prologue: None,
        },
        Pass {
            label: "usvg::Group",
            target: "Resvg",
            files: &usvg_files,
            ty: "Group",
            receiver: quote!(self.tree.root()),
            skip: &["isolate", "should_isolate", "id"],
            prologue: None,
        },
        Pass {
            label: "usvg::Node",
            target: "SvgNode",
            files: &usvg_files,
            ty: "Node",
            receiver: quote!(__n),
            skip: &["subroots"],
            prologue: Some(&node_prologue),
        },
    ];

    let mut generated: BTreeMap<&str, TokenStream> = BTreeMap::new();
    let mut taken: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    // Reported once the template exists: whether a skipped method is "covered"
    // is a question about the template, which is built further down.
    let mut skips: Vec<(&str, Vec<String>)> = Vec::new();
    for p in &passes {
        let names = taken.entry(p.target).or_default();
        let (code, skipped) = map_methods(
            &MethodPass {
                files: p.files,
                ty: p.ty,
                receiver: &p.receiver,
                skip: p.skip,
                prologue: p.prologue,
                readonly: false,
            },
            &vocab,
            names,
        );
        skips.push((p.label, skipped));
        generated.insert(p.ty, code);
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
            let used = |t: &TokenStream| t.to_string().contains(&c);
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
        // Plain `contains`, not the space-padded `mentions` used for type names:
        // a token stream renders a call as `foo_to_js(..)` with no space before
        // the paren, and a `_to_js` identifier is specific enough that a
        // substring cannot collide.
        let conv = info.conv_ident(name).to_string();
        let used = |c: &TokenStream| c.to_string().contains(&conv);
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

    let fontdb_methods = &generated["Database"];
    let tree_methods = &generated["Tree"];
    let group_methods = &generated["Group"];
    let node_methods = &generated["Node"];

    for s in skipped_fields {
        report!("usvg::Options field not exposed: {s}");
    }
    for s in skipped_write {
        report!("usvg::WriteOptions field not exposed: {s}");
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
    for (from, to) in RENAMED {
        assert!(
            defined.iter().any(|n| n.as_str() == *to),
            "RENAMED says `{from}` is covered by `{to}`, but the template defines no `{to}`"
        );
    }
    for (label, skipped) in &skips {
        for s in skipped {
            let name = s.split(' ').next().unwrap_or_default();
            let renamed = RENAMED.iter().find(|(from, _)| *from == name);
            let verb = match renamed {
                Some((_, to)) => format!("covered by the template as `{to}`"),
                None if defined.iter().any(|n| n.as_str() == name) => {
                    "covered by the template, not generated".to_string()
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

    const HEADER: &str = concat!(
        "// @generated by build.rs from the resvg/usvg/fontdb sources. DO NOT EDIT.\n",
        "// Regenerate with `touch build.rs && cargo build`.\n",
        "#![allow(clippy::all, dead_code)]\n",
    );
    let text = format!("{HEADER}{}", code);

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
