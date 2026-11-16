//! The whole pipeline, in order: what a build script runs.
//!
//! The other phases each answer one question; this one asks them in the
//! order their answers depend on -- the crates read and parsed, the
//! vocabulary, the classes, the enums, the fixpoints over data types and
//! handles, the option objects, the unions -- and writes the binding crate's
//! `src/lib.rs` and the `.d.ts` header napi prepends (`dts-header.d.ts`).
//! Everything specific to the wrapped crates is in the [`Config`] and the
//! template the build script passes.
//!
//! The split is verifiable rather than a matter of taste: the generated
//! files are committed and CI diffs them, so a refactor here is correct
//! exactly when the output does not move.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::Item;

use crate::class::{Classes, Source as ClassSource};
use crate::emit::*;
use crate::shape::*;
use crate::sources::*;
use crate::vocab::*;
use crate::{Config, Role};

/// What [`generate`] did, beside the files it wrote.
pub struct Output {
    /// The codegen's report, one line each: what it derived from upstream,
    /// and what it left alone with the reason. Printed as cargo warnings too
    /// when [`Config::log_env`] is set; here whether or not, for a build
    /// script to assert on.
    pub report: Vec<String>,
}

/// The report: kept, and printed when [`Config::log_env`] asks for it.
struct Log {
    on: bool,
    lines: Vec<String>,
}

impl Log {
    fn line(&mut self, line: String) {
        if self.on {
            println!("cargo::warning={line}");
        }
        self.lines.push(line);
    }
}

/// `say!(log, "{x} skipped")`: one report line.
macro_rules! say {
    ($log:expr, $($arg:tt)*) => {
        $log.line(format!($($arg)*))
    };
}

/// One [`Role::Discover`] crate, read through its own module tree.
///
/// `root_str` is kept beside `root`: quote! spells a path with spaces around
/// `::`, and this one reaches the report a reader is expected to grep.
struct Discovered {
    /// Its public files, but a module's with a namespace of its own.
    files: Vec<syn::File>,
    /// The same, by path.
    parsed: Vec<(PathBuf, syn::File)>,
    tree: ModTree,
    /// Its class sources: the root module first.
    scopes: Vec<Scope>,
    root: TokenStream,
    root_str: &'static str,
}

/// One [`Role::Classes`] crate: its packages read as one source.
struct ClassCrate {
    files: Vec<syn::File>,
    lib: Vec<syn::File>,
    modules: BTreeMap<String, String>,
    root: &'static str,
}

/// A source of classes and functions inside a discovered crate: its root
/// module, or a module with a namespace of its own (`Crate::modules`).
struct Scope {
    root: String,
    files: Vec<syn::File>,
    /// Where the free functions are: the crate's `lib.rs`, or the module's
    /// files.
    lib: Vec<syn::File>,
    /// `ModTree::paths` for the root module, none for a module of its own.
    modules: BTreeMap<String, String>,
    /// A module with a namespace of its own, rather than a crate's root.
    module: bool,
}

/// Runs every phase over the crates `config` lists and writes the binding
/// crate's `src/lib.rs` and `dts-header.d.ts` (napi's `dtsHeaderFile`),
/// each only when it changed. `head` is the build script's own items, put
/// after the `use`s and hand-written modules and before the generated ones
/// (the converters `Config::converters` names, say); `tail` goes after
/// everything. Panics, failing the build, on what upstream no longer has.
pub fn generate(config: &Config, head: TokenStream, tail: TokenStream) -> Output {
    let config = *config;
    let mut log = Log {
        on: env::var_os(config.log_env).is_some(),
        lines: Vec::new(),
    };
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // The generated file is also an *input* to watch: if someone deletes or
    // stubs src/lib.rs, the script must re-run and regenerate it.
    println!("cargo::rerun-if-changed=src/lib.rs");
    for h in config.hand_written {
        println!("cargo::rerun-if-changed={}", h.path);
    }
    println!("cargo::rerun-if-env-changed={}", config.log_env);

    let locked = locked_versions();

    // The crates this generator derives from (`Role::Discover`), in the order
    // a bare name is resolved against them. The point of the list is that a
    // third is a row rather than another arm in five `if` chains -- the bare
    // name decided the crate in every one of them, which is the assumption
    // that breaks the moment two crates define `Color`.
    //
    // Whole crates, no per-file list: the `impl` blocks wrapped are spread
    // over many modules. Read through each crate's own module tree: which
    // types are public, and at which path, is what its `mod`s and `pub use`s
    // say.
    let sources: Vec<Discovered> = config
        .crates_of(Role::Discover)
        .enumerate()
        .map(|(i, c)| {
            let [pkg] = c.pkgs else {
                panic!("Config::crates: {} is discovered, so one package", c.root)
            };
            let dir = locate(pkg, "lib.rs", &locked);
            if i == 0 {
                say!(log, "{} sources: {}", c.root, dir.display());
            }
            let mut all = parse_crate(&dir);
            // What it re-exports of a dependency's enums is its own
            // (`a::Shade`, defined by another crate).
            let mut load = |krate: &str| dependency_files(krate, &locked);
            for name in inline_reexported_enums(&mut all, &mut load) {
                say!(log, "{} re-exports enum {name}: read as its own", c.root);
            }
            let tree = ModTree::load(&dir, &all);
            // A module with a namespace of its own (`Crate::modules`) is a
            // class source of its own, and not walked: its listed classes,
            // functions and constants are emitted, nothing is discovered in
            // it. ponytail: discovery over such a module waits for a crate
            // that needs it.
            let ns_of = |p: &PathBuf| tree.file_ns.get(p).map_or("", String::as_str);
            let (own, apart): (Vec<_>, Vec<_>) = tree
                .public_view(&all, &dir)
                .into_iter()
                .partition(|(p, _)| !c.modules.iter().any(|(m, _)| *m == ns_of(p)));
            let in_ns = |files: &[(PathBuf, syn::File)], ns: &str| -> Vec<syn::File> {
                files
                    .iter()
                    .filter(|(p, _)| ns_of(p) == ns)
                    .map(|(_, f)| f.clone())
                    .collect()
            };
            // The crate's root module, and each module with a namespace of
            // its own. A crate's classes are its root module's: a module whose
            // types are exported under its own path (`filter`) is not that
            // source, and its `Image` is not the root's.
            let lib = own
                .iter()
                .filter(|(p, _)| *p == dir.join("lib.rs"))
                .map(|(_, f)| f.clone())
                .collect();
            let scopes = std::iter::once(Scope {
                root: c.root.to_string(),
                files: in_ns(&own, ""),
                lib,
                modules: tree.paths.clone(),
                module: false,
            })
            .chain(c.modules.iter().map(|(m, _)| {
                let files = in_ns(&apart, m);
                Scope {
                    root: format!("{}::{m}", c.root),
                    lib: files.clone(),
                    files,
                    modules: BTreeMap::new(),
                    module: true,
                }
            }))
            .collect();
            Discovered {
                files: own.iter().map(|(_, f)| f.clone()).collect(),
                parsed: own,
                tree,
                scopes,
                root: c
                    .root
                    .parse()
                    .expect("a Config::crates root that is a path"),
                root_str: c.root,
            }
        })
        .collect();
    // The primary crate: options and clamps are read from it.
    let primary = sources
        .first()
        .expect("Config::crates: at least one discovered crate");
    let primary_files = &primary.files;
    let modules = &primary.tree.paths;
    // Its files grouped by the namespace they export into. A bare-name lookup
    // over the whole crate takes whichever file was parsed first, which is how
    // a module's `Image` once came back with the root's `Image`'s methods; a
    // qualified key resolves against its own module instead.
    let mut by_module: BTreeMap<String, Vec<syn::File>> = BTreeMap::new();
    for (path, file) in &primary.parsed {
        let m = primary.tree.file_ns.get(path).cloned().unwrap_or_default();
        by_module.entry(m).or_default().push(file.clone());
    }

    // The class sources inside the discovered crates.
    let scopes: Vec<&Scope> = sources.iter().flat_map(|s| &s.scopes).collect();

    // Which crate defines a type, by the same probe the chains used: the first
    // source with a public struct of that name.
    let defines = |t: &str| {
        sources
            .iter()
            .find(|s| struct_fields_opt(&s.files, bare(t)).is_some())
    };
    // Whether a source other than `root` defines `name`: a class keeps its
    // upstream name only when none does, whichever source is probed first.
    // A module with a namespace of its own is such a source too.
    let defined_elsewhere = |name: &str, root: &str| {
        sources
            .iter()
            .any(|s| s.root_str != root && struct_fields_opt(&s.files, name).is_some())
            || scopes
                .iter()
                .any(|s| s.module && s.root != root && struct_fields_opt(&s.files, name).is_some())
    };

    // Enums count as well as structs, and they count together: `payload_enums`
    // qualifies a carried type by this set, so an enum defined in two modules --
    // or an enum in one and a struct of the same name in another -- would be
    // keyed by its bare name and resolve to whichever the walk reached first.
    // That is the failure a module's `Image` already caused once on the struct
    // side.
    let dups: BTreeSet<String> = {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        // Across every source, not within the primary alone. The count is
        // what decides whether a name is keyed by its module, and a name
        // defined once in each of two crates resolves to whichever was walked
        // first -- the same failure, one level up.
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
        say!(
            log,
            "names defined more than once across the sources, keyed by module: {}",
            dups.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }

    // Each index-bounded field's highest usable index, read off its table.
    let bounds = index_bounds(primary_files, &config);
    for ((suffix, table), (_, max)) in config.index_bounded.iter().zip(&bounds) {
        say!(
            log,
            "{} index bound for `*{suffix}` derived from {table}: {max}",
            primary.root_str
        );
    }

    // Enums to mirror = named by a config field OR by a return of a wrapped impl.
    let mut referenced: Types = config
        .option_objects
        .iter()
        .flat_map(|o| {
            defines(o)
                .map(|s| struct_field_types(&s.files, o))
                .unwrap_or_default()
        })
        .collect();
    referenced.extend(returned_types(primary_files));
    for src in &sources[1..] {
        referenced.extend(returned_types(&src.files));
        referenced.extend(field_types(&src.files));
    }

    // Object candidates: a *collection* of values wants a value type, whereas a
    // single borrow wants a handle. So `&[Stop]` and `impl Iterator<Item=&Info>`
    // make Stop/Info objects, while `&Group` stays a handle question.
    let object_seeds: BTreeSet<String> = referenced
        .values()
        .filter(|t| matches!(Shape::of(unref(t)), Shape::Slice(_) | Shape::ImplIter(_)))
        .map(|t| ty_str(peel(t)))
        // Plus what a payload enum carries. Safe on two counts: a duplicated
        // name is keyed by its module, and a member returning a node type
        // does not drag the tree in behind it.
        .chain(
            by_module
                .iter()
                .flat_map(|(m, files)| payload_enums(files, m, &dups, primary.root_str, &config))
                .flat_map(|(_, p)| p.variants)
                .flat_map(|(_, payload)| payload.carried().into_iter().filter_map(key).collect::<Vec<_>>())
                .filter(|t| !config.node_types.contains(&bare(t)))
                .filter(|t| struct_fields_opt(primary_files, bare(t)).is_some()),
        )
        // Plus the projected structs (`Config::projections`): a node payload
        // is not a collection, so nothing else nominates one, and what it
        // holds is data of the projection class. The projection is
        // `crate::project`'s; its data type only reaches what it holds.
        .chain(
            config
                .projections
                .iter()
                .map(|(_, t)| t.to_string())
                .filter(|t| {
                    // Not one already claimed as a handle: an `Arc`-held
                    // definition would get `wrap` twice.
                    let arc_held = referenced.values().any(|r| {
                        matches!(Shape::of(peel(r)), Shape::Arc(a) if key(a).as_deref() == Some(t.as_str()))
                    });
                    !arc_held
                        && !config.node_types.contains(&t.as_str())
                        && sources
                            .iter()
                            .any(|s| struct_fields_opt(&s.files, t).is_some())
                }),
        )
        .collect();

    let mut vocab = Vocab {
        config,
        ..Vocab::default()
    };
    // The vocabulary-only crates (`Role::Vocabulary`), each read as one set.
    let extra: Vec<(Vec<syn::File>, &str)> = config
        .crates_of(Role::Vocabulary)
        .map(|c| {
            let files = c
                .pkgs
                .iter()
                .flat_map(|p| parse_crate(&locate(p, "lib.rs", &locked)))
                .map(|(_, f)| f)
                .collect();
            (files, c.root)
        })
        .collect();
    // Each set with the crate path its types are reached by. A vocabulary
    // crate is mined for vocabulary only -- what a wrapped crate re-exports
    // of it is reachable through that crate, and what it does not re-export
    // is never emitted, so the crate's root is the honest label for the half
    // that can appear.
    for (set, root) in sources
        .iter()
        .map(|s| (&s.files, s.root_str))
        .chain(extra.iter().map(|(f, r)| (f, *r)))
    {
        // A vocabulary crate is parsed for exactly this kind of answer: a
        // wrapped crate re-exports its types, so the struct that decides the
        // mapping is never in the wrapped crate's own files.
        vocab.scalars.extend(f32_newtypes(set));
        vocab.aliases.extend(type_aliases(set));
        vocab
            .payload
            .extend(payload_enums(set, "", &dups, root, &config));
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
            .extend(payload_enums(files, m, &dups, primary.root_str, &config));
    }
    say!(
        log,
        "payload enums found: {}",
        vocab
            .payload
            .iter()
            .map(|(n, p)| format!("{n}({})", p.variants.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    say!(
        log,
        "vocabulary derived: {} f32 newtypes, {} type aliases",
        vocab.scalars.len(),
        vocab.aliases.len()
    );

    // The classes and free functions (`Config::classes`, `Config::functions`):
    // over the discovered crates, and over the crates read for them alone
    // (`Role::Classes`), each read as one source. The crate root's files are
    // where its free functions are. Built before the data types: a member
    // naming a class held by value is that class.
    let extra_crates: Vec<ClassCrate> = config
        .crates_of(Role::Classes)
        .map(|c| {
            let (mut files, mut lib, mut modules) = (Vec::new(), Vec::new(), BTreeMap::new());
            for pkg in c.pkgs {
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
                root: c.root,
            }
        })
        .collect();
    let classes = Classes::new(
        scopes
            .iter()
            .map(|s| ClassSource {
                root: &s.root,
                files: &s.files,
                modules: &s.modules,
            })
            .chain(extra_crates.iter().map(|c| ClassSource {
                root: c.root,
                files: &c.files,
                modules: &c.modules,
            }))
            .collect(),
        config,
    );
    // By its upstream name, unless another source defines that name (`a::Color`
    // is not `b.Color`). A discovered crate's own class is the type its
    // members name.
    for (name, plain, root) in classes.by_value() {
        let enum_here = sources.iter().any(|s| find_enum(&s.files, name).is_some());
        if !defined_elsewhere(name, root) && !enum_here {
            vocab.classes.insert(name.to_string());
            if plain {
                vocab.plain.insert(name.to_string());
            }
        }
    }
    for (name, root) in classes.behind_arc() {
        if !defined_elsewhere(name, root) {
            vocab.shared.insert(name.to_string());
        }
    }
    // The classes a borrowed member hands out (a tree in an image, say), under
    // the same rule: a copy, or, for a projected one reached from a
    // projection, the value borrowed in place.
    for (name, root) in classes.by_clone() {
        if !defined_elsewhere(name, root) {
            vocab.owned.insert(name.to_string());
        }
    }
    for (name, root) in classes.by_projection() {
        if !defined_elsewhere(name, root) {
            vocab.projected.insert(name.to_string());
        }
    }
    say!(
        log,
        "classes a borrowing member copies: {}",
        vocab
            .owned
            .difference(&vocab.projected)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    say!(
        log,
        "classes a borrowing member borrows from a projection, copies otherwise: {}",
        vocab
            .projected
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Every source, not just the primary: another crate defines enums too, and
    // a primary-only pass left them to the object path, which reported them as
    // "not a public struct" and dropped the field that used them. Driven by
    // the list so a third crate is a push.
    let mut enum_names: BTreeSet<String> = BTreeSet::new();
    // The surface, by the root of the source that defines each item:
    // `crate::emit::namespaced` puts it under that source's namespace.
    let mut mirrored: BTreeMap<String, TokenStream> = BTreeMap::new();
    let mut put = |root: &str, code: TokenStream| {
        mirrored.entry(root.to_string()).or_default().extend(code);
    };
    for src in &sources {
        let (names, code, rejected) = map_enums(
            &src.files,
            &referenced.keys().cloned().collect(),
            modules,
            &src.root,
        );
        enum_names.extend(names);
        put(src.root_str, code);
        for (name, why) in rejected {
            say!(log, "payload enum {name} not mapped: {why}");
        }
    }
    // `enum_names` already carries every source's enums.
    vocab.enums = enum_names.clone();
    for src in &sources {
        vocab.ints.extend(int_newtypes(&src.files));
    }

    // Fixpoint over object types: a generated object can reference another
    // (`Stop` holds a `Color`).
    // Plus what a listed class of a discovered crate returns: the class maps
    // a member returning a data type only when that type is one. A struct of
    // a discovered crate, not itself a class, a node or an iterated list.
    let class_returns: Vec<String> = classes
        .classes
        .iter()
        .filter(|c| {
            scopes
                .iter()
                .any(|s| s.root == classes.sources[c.source].root)
        })
        .flat_map(|c| inherent_impls(classes.sources[c.source].files, &c.name))
        .flat_map(|imp| &imp.items)
        .filter_map(|it| match it {
            syn::ImplItem::Fn(f) if is_pub(&f.vis) => match &f.sig.output {
                syn::ReturnType::Type(_, t) => Some(&**t),
                syn::ReturnType::Default => None,
            },
            _ => None,
        })
        .flat_map(peeled)
        .filter_map(key)
        .filter(|t| {
            !object_seeds.contains(t)
                && !vocab.enums.contains(t)
                && !config.node_types.contains(&t.as_str())
                && !config.projections.iter().any(|(_, p)| p == t)
                && !config.iterated.iter().any(|it| it.ty == t)
                && !classes.classes.iter().any(|c| c.name == *t)
                && defines(t).is_some()
        })
        .collect();
    let mut object_todo: VecDeque<String> = object_seeds.into_iter().chain(class_returns).collect();
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
        // A listed class of a discovered crate is `crate::class`'s, whatever
        // reaches it (a payload carrying it, a list of it): emitted twice, the
        // two would clash. By its bare name only: a module's type of the same
        // name (`m::Image`) is another one. A projection is walked as data
        // (what it holds is data of its class), and left out when emitted.
        let listed = |s: &Discovered| config.classes.contains(&(s.root_str, t.as_str()));
        if !t.contains("::") && defines(&t).is_some_and(listed) {
            say!(log, "object {t} skipped: a listed class");
            continue;
        }
        let scoped = t.rsplit_once("::").and_then(|(m, _)| by_module.get(m));
        let (source, root) = if let Some(files) = scoped {
            (files, primary.root.clone())
        } else if let Some(src) = defines(&t) {
            (&src.files, src.root.clone())
        } else {
            say!(log, "object {t} skipped: not a public struct");
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
        // dependencies it would have queued never were. A decoration that
        // reaches only its style was reported as having nothing mappable,
        // and the pair stayed invisible.
        let mut seeded = false;
        // A dropped member whose type is itself a public struct is a candidate:
        // that is how `Stop` reaches `Color`.
        for d in &dropped {
            let bare =
                d.ty.as_ref()
                    .map(|t| ty_str(peel(t)))
                    .unwrap_or_default()
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
            say!(log, "data type {t} skipped: nothing mappable on it");
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
                say!(
                    log,
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
    for t in &vocab.values {
        // The dropped list is the *class's* own: a getter maps members an
        // object field cannot, so reporting the object probe's list here would
        // name members that are in fact exposed.
        let (vcode, dropped, _) = value_class(source_of(t), t, &vocab, modules, &object_root[t]);
        object_parts.insert(t.clone(), vcode);
        say!(
            log,
            "{t}: partial mapping, emitted as read-only class {}",
            data_ident(t)
        );
        for d in dropped {
            say!(log, "{t} member not exposed: {d}");
        }
    }

    // The option structs as plain objects (`Config::option_objects`): every
    // field optional over `Default`, a field with no JS form supplied by a
    // provider or converted by a converter. Each from the source that defines
    // it, the primary first.
    let mut clamped: BTreeSet<&str> = BTreeSet::new();
    for name in config.option_objects {
        let src = defines(name).unwrap_or(primary);
        let Some(obj) = option_object(
            &src.files,
            name,
            &vocab,
            &src.tree.paths,
            &src.root,
            &bounds,
        ) else {
            say!(
                log,
                "{}::{name} not an option object: no Default to fill it from",
                src.root_str
            );
            continue;
        };
        clamped.extend(obj.bounded);
        for s in obj.skipped {
            say!(
                log,
                "{}::{name} field not in the option object: {s}",
                src.root_str
            );
        }
        put(src.root_str, obj.code);
    }
    // The clamped indices (`Config::index_bounded`) are found by a name
    // suffix. A rename upstream would match nothing, fall through to
    // `u8::MAX`, and hand upstream an index far past the end of its table --
    // an out-of-bounds panic, inside an `extern "C"` callback if upstream
    // indexes it there, which aborts the process rather than throwing. Loud
    // here instead.
    for (suffix, table) in config.index_bounded {
        assert!(
            clamped.contains(suffix),
            "no option field matched the index bound `*{suffix}` ({table}): either upstream \
             renamed it, in which case the clamp now protects nothing, or it is gone"
        );
    }

    // Enums JS passes in, borrowed for the call: their `{ type, value }` input
    // form.
    for src in &sources {
        for e in input_enums(&src.files, &config) {
            say!(
                log,
                "{}::{} is an input enum: {{ type, value }}",
                src.root_str,
                e.ident
            );
            put(
                src.root_str,
                enum_input(e, &src.tree.paths, &src.root, &config),
            );
        }
    }

    // Fixpoint: start from the handle types the wrapped impls return, generate a
    // class for each, then follow the handles *those* classes return. The type
    // graph is finite, and `done` keeps a type from being visited twice.
    let handles_of = |set: &Types| -> BTreeSet<String> {
        set.values()
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
        let src = defines(&t).unwrap_or(primary);
        let key = src
            .root_str
            .rsplit("::")
            .next()
            .and_then(|last| t.strip_prefix(&format!("{last}::")))
            .filter(|_| src.root_str.contains("::"))
            .unwrap_or(&t);
        // A class `Config::classes` lists is `crate::class`'s, below.
        // So is a projection (`Config::projections`), by its full key: a
        // module's type of the same name is another one.
        let listed = config
            .classes
            .iter()
            .any(|(r, n)| *r == src.root_str && *n == bare(key))
            || config
                .projections
                .iter()
                .any(|(r, n)| *r == src.root_str && *n == key);
        if listed {
            continue;
        }
        let (code, skipped, reached) = wrapper_class(&src.files, key, &vocab, modules, &src.root);
        for s in skipped {
            say!(
                log,
                "{}::{key} method not in the namespaced class: {s}",
                src.root_str
            );
        }
        put(src.root_str, code);
        todo.extend(reached.into_iter().filter(|x| !done.contains(x)));
    }
    say!(
        log,
        "wrapper classes generated: {}",
        done.iter().cloned().collect::<Vec<_>>().join(", ")
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
        say!(log, "{root}::{} is a class: {what}", c.name);
        for s in skipped {
            say!(log, "{root}::{} member not in the class: {s}", c.name);
        }
        put(root, code);
    }
    let libs = scopes
        .iter()
        .map(|s| (s.root.as_str(), &s.lib))
        .chain(extra_crates.iter().map(|c| (c.root, &c.lib)));
    for (root, lib) in libs {
        let (code, skipped) = classes.functions(root, lib, &vocab);
        for s in skipped {
            say!(log, "{root} function not exposed: {s}");
        }
        put(root, code);
    }
    // The constants the discovered crates export, each in the namespace of
    // the module exporting it.
    for src in &sources {
        let (code, lines) = constants(src.root_str, &src.tree.consts, &vocab);
        for line in lines {
            say!(log, "{line}");
        }
        for (root, code) in code {
            put(&root, code);
        }
    }
    for u in &classes.unions {
        let root = classes.sources[u.source].root;
        say!(log, "{root}::{} is a union of projections", u.name);
    }
    for (s, code) in classes
        .error_codes()
        .into_iter()
        .chain(classes.union_code())
    {
        put(classes.sources[s].root, code);
    }

    // Every settled data type, objects and read-only classes alike: the
    // registry only ever holds what a member or another data type reached.
    for (t, code) in &object_parts {
        let root = object_root[t].to_string().replace(' ', "");
        let src = sources
            .iter()
            .find(|s| s.root_str == root)
            .expect("a data type's root is a source's");
        // A type `Config::projections` lists is `crate::project`'s class.
        if config
            .projections
            .iter()
            .any(|(r, n)| *r == src.root_str && *n == t.as_str())
        {
            say!(log, "{root}::{t} is a projection, not a data type");
            continue;
        }
        put(src.root_str, code.clone());
    }

    // The unions for the payload enums. Generated last, because a payload may
    // be one of the data types settled just above.
    let mut unions: Vec<String> = Vec::new();
    for (name, info) in &vocab.payload {
        if let Some(why) = info.payload_blocker(&vocab) {
            say!(log, "payload enum {name} not mapped: {why}");
            continue;
        }
        match payload_enum_code(name, info, &vocab, modules) {
            Ok(code) => {
                // In the namespace of the source defining the enum; one mined
                // from a vocabulary crate, in the namespace of the path that
                // reaches it.
                let src = sources
                    .iter()
                    .find(|s| find_enum(&s.files, bare(name)).is_some());
                let root = src.map_or(info.root.as_str(), |s| s.root_str);
                if config.namespace_of(root).is_none() {
                    say!(
                        log,
                        "payload enum {name} not namespaced: no namespace for {root}"
                    );
                    continue;
                }
                put(root, code);
                unions.push(name.clone());
            }
            Err(why) => say!(log, "payload enum {name} skipped: {why}"),
        }
    }
    if !unions.is_empty() {
        say!(
            log,
            "payload enums emitted as unions: {}",
            unions.join(", ")
        );
    }
    say!(
        log,
        "data types emitted: {}",
        object_parts.keys().cloned().collect::<Vec<_>>().join(", ")
    );

    // The surface: one namespace per source (`Config::crates`), after the
    // generic prelude, the hand-written modules and the template's own items.
    let parts: Vec<(&str, TokenStream)> = mirrored
        .into_iter()
        .map(|(root, code)| {
            let ns = config
                .namespace_of(&root)
                .unwrap_or_else(|| panic!("no Config::crates namespace for {root}"));
            (ns, code)
        })
        .collect();
    let surfaces = namespaced(&parts, &classes.shared_names());
    for line in surfaces.report {
        say!(log, "{line}");
    }
    let (prelude, namespaced) = (prelude(), surfaces.namespaced);
    let uses = config.uses.iter().map(|u| {
        let path: TokenStream = u.parse().expect("a Config::uses path");
        quote!(use #path;)
    });
    let hand_written: Vec<(String, syn::File)> = config
        .hand_written
        .iter()
        .map(|h| {
            let path = manifest.join(h.path);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_else(|| panic!("Config::hand_written: {} names no file", h.path))
                .to_string();
            let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", h.path));
            let file = syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", h.path));
            (stem, file)
        })
        .collect();
    let mods = config
        .hand_written
        .iter()
        .zip(&hand_written)
        .map(|(h, (stem, file))| {
            let doc = format!(" {}", h.doc);
            let id = format_ident!("{stem}");
            // The classes of it a provider names, which the generated code names
            // bare.
            let provided: BTreeSet<&str> = config
                .providers
                .iter()
                .map(|p| p.class)
                .filter(|n| {
                    file.items
                        .iter()
                        .any(|i| matches!(i, Item::Struct(s) if s.ident == n && is_pub(&s.vis)))
                })
                .collect();
            let provided: Vec<_> = provided.into_iter().map(|n| format_ident!("{n}")).collect();
            let import = (!provided.is_empty()).then(|| quote!(use #id::{#(#provided),*};));
            quote! {
                #[doc = #doc]
                mod #id;
                #import
            }
        });
    let code = {
        let mut file: syn::File = syn::parse2(quote! {
            #prelude
            #(#uses)*
            #(#mods)*
            #head
            #namespaced
            #tail
        })
        .expect("the generated file is valid Rust");
        // `dispose()` on every class, and `[Symbol.dispose]` with its
        // declaration (napi's `dtsHeaderFile`).
        crate::dispose::slotted(&mut file);
        // The hand-written modules' classes get `[Symbol.dispose]` and
        // `util.inspect` like the generated ones: the two passes read them
        // beside the file, which does not hold them.
        let mut whole = file.clone();
        for (_, m) in &hand_written {
            whole.items.extend(m.items.iter().cloned());
        }
        let (alias, dts_header) = crate::dispose::aliases(&whole);
        write_if_changed(&manifest.join(DTS_HEADER), &dts_header);
        let inspect = inspect_members(&whole);
        quote! { #file #alias #inspect }
    };

    let header = format!(
        "// @generated by build.rs from {}. DO NOT EDIT.\n{}",
        config.generated_from, HEADER_REST
    );
    let text = format!("{header}{code}");

    // Staging copy, handy for inspection: $OUT_DIR/bindings.rs
    let staged = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    fs::write(&staged, &text).unwrap();
    // rustfmt follows `mod x;` and gives up when the file is not beside the
    // staged copy: an empty stand-in is enough.
    for (stem, _) in &hand_written {
        fs::write(staged.with_file_name(format!("{stem}.rs")), "").unwrap();
    }
    rustfmt(&staged);

    // Real target. It has to be the crate root itself: `napi-derive` keeps a
    // global struct registry and requires `#[napi] struct` to expand before
    // `#[napi] impl`, which `include!(...)` does not preserve.
    let formatted = fs::read_to_string(&staged).unwrap_or(text);
    write_if_changed(&manifest.join("src/lib.rs"), &formatted);
    Output { report: log.lines }
}

/// The generated file's header after its first line.
const HEADER_REST: &str = concat!(
    "// Regenerate with `touch build.rs && cargo build`.\n",
    "#![allow(dead_code)]\n",
    // Generated by type text, not type checking: an upstream alias may
    // already be the target type, option structs are filled field by
    // field from Default, and a 17-variant Either is what napi offers for
    // a JS union; a `new` is a JS constructor, so a Default impl would be
    // dead API. Everything else clippy says applies here too.
    "#![allow(clippy::useless_conversion, clippy::field_reassign_with_default, clippy::type_complexity, clippy::new_without_default)]\n",
);

/// The `.d.ts` header napi writes first (`napi.dtsHeaderFile` in
/// `package.json`): generated, committed, drift-checked.
const DTS_HEADER: &str = "dts-header.d.ts";

/// Writes only on change: a touched mtime re-triggers the build.
fn write_if_changed(path: &Path, text: &str) {
    if fs::read_to_string(path).ok().as_deref() != Some(text) {
        fs::write(path, text).unwrap();
    }
}

fn rustfmt(path: &Path) {
    // Best effort: readable output when rustfmt is around, ignored otherwise.
    let _ = std::process::Command::new("rustfmt")
        .arg("--edition=2024")
        .arg(path)
        .status();
}
