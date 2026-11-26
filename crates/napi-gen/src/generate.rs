//! The whole pipeline, in order: what a build script runs.
//!
//! The other phases each answer one question; this one asks them in the
//! order their answers depend on -- the crates read and parsed, the
//! vocabulary, the classes, the enums, the fixpoints over data types and
//! handles, the option objects, the unions -- and writes the binding crate's
//! `src/lib.rs` and the `.d.ts` header napi prepends (`dts-header.d.ts`).
//! Each step is a function of its own ([`discover`], [`settle_classes`],
//! [`vocabulary`], [`data_types`], the emitting ones, [`assemble`]): what
//! one settles reaches the next as its return value, so [`generate`] reads
//! as the order and nothing else. Everything specific to the wrapped crates is in the [`Config`] and the
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
    /// The types a caller reaches by the crate's root path: what its first
    /// package exports, its own and what its `lib.rs` re-exports by name of
    /// the others. A later package's export the first does not re-export (an
    /// internal helper type) is not one.
    exported: BTreeSet<String>,
    /// The constants its first package exports ([`ModTree::consts`]), their
    /// types read through its type aliases: emitted as a discovered crate's
    /// are (`emit::constants`).
    consts: Vec<(String, String, syn::ItemConst)>,
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

/// The [`Role::Discover`] crates, read: the phase every other one starts from.
struct Discovery {
    /// In the order a bare name is resolved against them; the primary first.
    sources: Vec<Discovered>,
    /// The primary's files grouped by the namespace they export into. A
    /// bare-name lookup over the whole crate takes whichever file was parsed
    /// first, which is how a module's `Image` once came back with the root's
    /// `Image`'s methods; a qualified key resolves against its own module
    /// instead.
    by_module: BTreeMap<String, Vec<syn::File>>,
    /// The names defined more than once across the sources, keyed by module.
    dups: BTreeSet<String>,
}

impl Discovery {
    /// The primary crate: options and clamps are read from it.
    fn primary(&self) -> &Discovered {
        self.sources
            .first()
            .expect("Config::crates: at least one discovered crate")
    }

    /// The class sources inside the discovered crates.
    fn scopes(&self) -> Vec<&Scope> {
        self.sources.iter().flat_map(|s| &s.scopes).collect()
    }

    /// Which crate defines a type, by the same probe the chains used: the
    /// first source with a public struct of that name.
    fn defines(&self, t: &str) -> Option<&Discovered> {
        self.sources
            .iter()
            .find(|s| struct_fields_opt(&s.files, bare(t)).is_some())
    }

    /// Whether a source other than `root` defines `name`: a class keeps its
    /// upstream name only when none does, whichever source is probed first.
    /// A module with a namespace of its own is such a source too.
    fn defined_elsewhere(&self, name: &str, root: &str) -> bool {
        self.sources
            .iter()
            .any(|s| s.root_str != root && struct_fields_opt(&s.files, name).is_some())
            || self
                .scopes()
                .iter()
                .any(|s| s.module && s.root != root && struct_fields_opt(&s.files, name).is_some())
    }
}

/// The generated items by the root of the source that defines each:
/// `crate::emit::namespaced` puts them under that source's namespace. Every
/// emitting phase adds to it, in the order the phases run.
#[derive(Default)]
struct Surface(BTreeMap<String, TokenStream>);

impl Surface {
    fn put(&mut self, root: &str, code: TokenStream) {
        self.0.entry(root.to_string()).or_default().extend(code);
    }
}

/// What the vocabulary phase settles for the phases after it.
struct Words {
    vocab: Vocab,
    /// The types named by a config field or by a return of a wrapped impl.
    referenced: Types,
    /// The data-type candidates the fixpoint over objects starts from.
    seeds: BTreeSet<String>,
    /// Each index-bounded field's highest usable index.
    bounds: Vec<(&'static str, u32)>,
}

/// The settled data types: objects and read-only classes alike.
struct DataTypes {
    /// Each one's code, by its key.
    parts: BTreeMap<String, TokenStream>,
    /// The crate path each one is reached by.
    root: BTreeMap<String, TokenStream>,
}

/// Runs every phase over the crates `config` lists and writes the binding
/// crate's `src/lib.rs` and `dts-header.d.ts` (napi's `dtsHeaderFile`),
/// each only when it changed. `head` is the build script's own items, put
/// after the `use`s and hand-written modules and before the generated ones
/// (the converters `Config::converters` names, say); `tail` goes after
/// everything. Panics, failing the build, on what upstream no longer has.
pub fn generate(config: &Config, head: TokenStream, tail: TokenStream) -> Output {
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
    let found = discover(config, &locked, &mut log);
    let extra_crates = class_crates(config, &locked);
    let classes = settle_classes(*config, &found, &extra_crates, &mut log);
    // From here on, the config with the derived classes and option objects.
    let config = classes.config;
    let mut surface = Surface::default();
    let mut words = vocabulary(&config, &found, &classes, &locked, &mut surface, &mut log);
    let data = data_types(&config, &found, &classes, &mut words, &mut log);
    let vocab = &words.vocab;
    option_objects(
        &config,
        &found,
        vocab,
        &words.bounds,
        &mut surface,
        &mut log,
    );
    emit_input_enums(&config, &found, &mut surface, &mut log);
    handle_classes(
        &config,
        &found,
        vocab,
        &words.referenced,
        &mut surface,
        &mut log,
    );
    let statics = listed_classes(
        &found,
        &extra_crates,
        &classes,
        vocab,
        &mut surface,
        &mut log,
    );
    emit_data_types(&config, &found, &data, &mut surface, &mut log);
    emit_unions(&config, &found, &classes, vocab, &mut surface, &mut log);
    say!(
        log,
        "data types emitted: {}",
        data.parts.keys().cloned().collect::<Vec<_>>().join(", ")
    );
    assemble(
        &config,
        &manifest,
        surface,
        &classes.shared_names(),
        statics,
        (head, tail),
        &mut log,
    );
    Output { report: log.lines }
}

/// The crates this generator derives from (`Role::Discover`), in the order
/// a bare name is resolved against them, and the names they define twice.
fn discover(config: &Config, locked: &BTreeMap<String, Vec<String>>, log: &mut Log) -> Discovery {
    // The point of the list is that a third is a row rather than another arm
    // in five `if` chains -- the bare name decided the crate in every one of
    // them, which is the assumption that breaks the moment two crates define
    // `Color`.
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
            let dir = locate(pkg, "lib.rs", locked);
            if i == 0 {
                say!(log, "{} sources: {}", c.root, dir.display());
            }
            let mut all = parse_crate(&dir);
            // What it re-exports of a dependency's enums is its own
            // (`a::Shade`, defined by another crate).
            let mut load = |krate: &str| dependency_files(krate, locked);
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
    let primary = sources
        .first()
        .expect("Config::crates: at least one discovered crate");
    let mut by_module: BTreeMap<String, Vec<syn::File>> = BTreeMap::new();
    for (path, file) in &primary.parsed {
        let m = primary.tree.file_ns.get(path).cloned().unwrap_or_default();
        by_module.entry(m).or_default().push(file.clone());
    }

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
    Discovery {
        sources,
        by_module,
        dups,
    }
}

/// The crates read for their classes alone (`Role::Classes`), each read as
/// one source. The crate root's files are where its free functions are.
fn class_crates(config: &Config, locked: &BTreeMap<String, Vec<String>>) -> Vec<ClassCrate> {
    config
        .crates_of(Role::Classes)
        .map(|c| {
            let (mut files, mut lib, mut modules) = (Vec::new(), Vec::new(), BTreeMap::new());
            let mut exported = BTreeSet::new();
            let mut consts = Vec::new();
            for (i, pkg) in c.pkgs.iter().enumerate() {
                let dir = locate(pkg, "lib.rs", locked);
                let all = parse_crate(&dir);
                let tree = ModTree::load(&dir, &all);
                for (path, file) in tree.public_view(&all, &dir) {
                    if path == dir.join("lib.rs") && lib.is_empty() {
                        exported.extend(reexported_names(&file));
                        lib.push(file.clone());
                    }
                    files.push(file);
                }
                if i == 0 {
                    exported.extend(tree.paths.keys().cloned());
                    consts = tree.consts.clone();
                    // Its type aliases are no vocabulary of the others' (it
                    // is read for its classes alone), but a constant's type
                    // may be one, exported or not (`pub const OPAQUE: Alpha
                    // = 255` with `pub type Alpha = u8` in a private
                    // module): read through them, so it maps as the aliased
                    // type does.
                    let files: Vec<syn::File> = all.iter().map(|(_, f)| f.clone()).collect();
                    let aliases = type_aliases(&files);
                    for (_, _, k) in &mut consts {
                        if let Some(to) = key(&k.ty).and_then(|t| aliases.get(&t)) {
                            *k.ty = to.clone();
                        }
                    }
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
                exported,
                consts,
            }
        })
        .collect()
}

/// Every class source: the discovered crates' scopes, then the
/// [`Role::Classes`] crates.
fn class_sources<'a>(found: &'a Discovery, extra_crates: &'a [ClassCrate]) -> Vec<ClassSource<'a>> {
    found
        .scopes()
        .into_iter()
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
        .collect()
}

/// The classes and free functions (`Config::classes`, `Config::functions`),
/// over the discovered crates and the [`Role::Classes`] ones, with the
/// config they settle: the value types the listed classes reach and the
/// option objects, both merged into its lists. Built before the data types:
/// a member naming a class held by value is that class.
fn settle_classes<'a>(
    config: Config,
    found: &'a Discovery,
    extra_crates: &'a [ClassCrate],
    log: &mut Log,
) -> Classes<'a> {
    let mut classes = Classes::new(class_sources(found, extra_crates), config);
    // The value types the listed classes reach (ADR 0001: the surface is
    // what the listed members reach), to a fixpoint: a `Copy` struct a
    // member of a class of a `Role::Classes` crate takes or returns is a
    // class too, held by value, and its own members reach further. Such a
    // crate is read for its classes alone, so nothing else gives the type a
    // JS form, and the member would be left out. A discovered crate finds
    // its data types on its own. Settled before anything reads the list, as
    // the option objects are below.
    let config = {
        let mut listed: Vec<(&'static str, &'static str)> = config.classes.to_vec();
        loop {
            let found_values = reached_values(&classes, extra_crates);
            if found_values.is_empty() {
                break;
            }
            for (root, name, by) in found_values {
                say!(
                    log,
                    "{root}::{name} reached by {by}: a class, held by value"
                );
                // A build script runs once: the merged list lives as long as it.
                listed.push((root, Box::leak(name.into_boxed_str())));
            }
            let config = Config {
                classes: Box::leak(listed.clone().into_boxed_slice()),
                ..config
            };
            classes = Classes::new(class_sources(found, extra_crates), config);
        }
        classes.config
    };
    // The option objects (`Config::option_objects`): what the listed members
    // take by `&T`, and what the config lists beside them. Settled before
    // anything reads the list.
    let config = {
        let all = derived_option_objects(&classes, &found.scopes());
        let listed = config.option_objects.iter().map(|o| o.to_string());
        let extra: Vec<String> = listed.filter(|o| !all.contains(o)).collect();
        // A build script runs once: the merged list lives as long as it.
        let names: Vec<&'static str> = all
            .into_iter()
            .chain(extra)
            .map(|n| &*Box::leak(n.into_boxed_str()))
            .collect();
        Config {
            option_objects: Box::leak(names.into_boxed_slice()),
            ..config
        }
    };
    classes.config = config;
    // A Role::Classes crate's constants are mirrored as a discovered
    // crate's are (`emit::constants`, ADR 0002), but of its types' associated
    // ones only its listed classes' (`statics::of_class`): the rest are said,
    // so their absence reads as a choice. A `Role::Vocabulary` crate is not
    // read through its module tree, so what it exports, constants included,
    // is never seen.
    for c in extra_crates {
        let consts = crate::statics::unexposed(&c.files, &c.exported, |t| classes.lists(c.root, t));
        if !consts.is_empty() {
            say!(
                log,
                "{} constants not exposed: {} (read only for its listed classes and functions)",
                c.root,
                consts.join(", ")
            );
        }
    }
    classes
}

/// The vocabulary: the index bounds, the types the config and the wrapped
/// impls name, the data-type seeds, what every crate's types map to, and the
/// mirrored enums (put on `surface`).
fn vocabulary(
    config: &Config,
    found: &Discovery,
    classes: &Classes,
    locked: &BTreeMap<String, Vec<String>>,
    surface: &mut Surface,
    log: &mut Log,
) -> Words {
    let (sources, by_module, dups) = (&found.sources, &found.by_module, &found.dups);
    let primary = found.primary();
    let primary_files = &primary.files;
    let modules = &primary.tree.paths;

    // Each index-bounded field's highest usable index, read off its table.
    let bounds = index_bounds(primary_files, config);
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
            found
                .defines(o)
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
    // single borrow wants a handle. So `&[Cell]` and `impl Iterator<Item=&Dot>`
    // make Cell/Dot objects, while `&Layer` stays a handle question.
    let seeds: BTreeSet<String> = referenced
        .values()
        .filter(|t| matches!(Shape::of(unref(t)), Shape::Slice(_) | Shape::ImplIter(_)))
        .map(|t| ty_str(peel(t)))
        // Plus what a payload enum carries. Safe on two counts: a duplicated
        // name is keyed by its module, and a member returning a node type
        // does not drag the tree in behind it.
        .chain(
            by_module
                .iter()
                .flat_map(|(m, files)| payload_enums(files, m, dups, primary.root_str, config))
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
        config: *config,
        ..Vocab::default()
    };
    // The vocabulary-only crates (`Role::Vocabulary`), each read as one set.
    let extra: Vec<(Vec<syn::File>, &str)> = config
        .crates_of(Role::Vocabulary)
        .map(|c| {
            let files = c
                .pkgs
                .iter()
                .flat_map(|p| parse_crate(&locate(p, "lib.rs", locked)))
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
        for (name, constness) in f32_newtypes(set) {
            if constness {
                vocab.const_scalars.insert(name.clone());
            }
            vocab.scalars.insert(name);
        }
        vocab.aliases.extend(type_aliases(set));
        vocab
            .payload
            .extend(payload_enums(set, "", dups, root, config));
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
    for (m, files) in by_module {
        if m.is_empty() {
            continue;
        }
        vocab
            .payload
            .extend(payload_enums(files, m, dups, primary.root_str, config));
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

    // By its upstream name, unless another source defines that name (`a::Color`
    // is not `b.Color`). A discovered crate's own class is the type its
    // members name.
    for (name, plain, root) in classes.by_value() {
        let enum_here = sources.iter().any(|s| find_enum(&s.files, name).is_some());
        if !found.defined_elsewhere(name, root) && !enum_here {
            vocab.classes.insert(name.to_string());
            if plain {
                vocab.plain.insert(name.to_string());
            }
        }
    }
    for (name, root) in classes.behind_arc() {
        if !found.defined_elsewhere(name, root) {
            vocab.shared.insert(name.to_string());
        }
    }
    // The classes a borrowed member hands out (a tree in an image, say), under
    // the same rule: a copy, or, for a projected one reached from a
    // projection, the value borrowed in place.
    for (name, root) in classes.by_clone() {
        if !found.defined_elsewhere(name, root) {
            vocab.owned.insert(name.to_string());
        }
    }
    for (name, root) in classes.by_projection() {
        if !found.defined_elsewhere(name, root) {
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
    for src in sources {
        let (names, code, rejected) = map_enums(
            &src.files,
            &referenced.keys().cloned().collect(),
            modules,
            &src.root,
        );
        enum_names.extend(names);
        surface.put(src.root_str, code);
        for (name, why) in rejected {
            say!(log, "payload enum {name} not mapped: {why}");
        }
    }
    // `enum_names` already carries every source's enums.
    vocab.enums = enum_names;
    for src in sources {
        vocab.ints.extend(int_newtypes(&src.files));
    }
    Words {
        vocab,
        referenced,
        seeds,
        bounds,
    }
}

/// The data types, to a fixpoint: which types are objects, which read-only
/// classes, and each one's code. Records both sets on `words.vocab`.
fn data_types(
    config: &Config,
    found: &Discovery,
    classes: &Classes,
    words: &mut Words,
    log: &mut Log,
) -> DataTypes {
    let (sources, by_module) = (&found.sources, &found.by_module);
    let primary = found.primary();
    let modules = &primary.tree.paths;
    let vocab = &mut words.vocab;
    // Fixpoint over object types: a generated object can reference another
    // (`Dot` holds a `Color`).
    // Plus what a listed class of a discovered crate returns: the class maps
    // a member returning a data type only when that type is one. A struct of
    // a discovered crate, not itself a class, a node or an iterated list.
    let scopes = found.scopes();
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
            !words.seeds.contains(t)
                && !vocab.enums.contains(t)
                && !config.node_types.contains(&t.as_str())
                && !config.projections.iter().any(|(_, p)| p == t)
                && !config.iterated.iter().any(|it| it.ty == t)
                && !classes.classes.iter().any(|c| c.name == *t)
                && found.defines(t).is_some()
        })
        .collect();
    let mut object_todo: VecDeque<String> = std::mem::take(&mut words.seeds)
        .into_iter()
        .chain(class_returns)
        .collect();
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
        if !t.contains("::") && found.defines(&t).is_some_and(listed) {
            say!(log, "object {t} skipped: a listed class");
            continue;
        }
        let scoped = t.rsplit_once("::").and_then(|(m, _)| by_module.get(m));
        let (source, root) = if let Some(files) = scoped {
            (files, primary.root.clone())
        } else if let Some(src) = found.defines(&t) {
            (&src.files, src.root.clone())
        } else {
            // Claimed by another mapping, it has a JS form already: a payload
            // enum (a union, or its own report line), a scalar, a handle's
            // `Arc`, a class. Only a type nothing maps is a gap.
            let claimed = vocab.payload.contains_key(&t)
                || syn::parse_str::<syn::Type>(&t)
                    .ok()
                    .and_then(|ty| vocab.classify(&ty))
                    .is_some();
            if !claimed {
                say!(log, "object {t} skipped: not a public struct");
            }
            continue;
        };
        vocab.objects.insert(t.clone());
        object_root.insert(t.clone(), root.clone());
        let (members, dropped, reached) = data_members(source, &t, vocab, true);
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
        // that is how `Dot` reaches `Color`.
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
    // wrong in registry order: `Line` sorts before `Pen`, so Line was built
    // holding `Option<Pen>` as an object field, and Pen was demoted a
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
        found
            .defines(t)
            .map(|s| s.files.as_slice())
            .unwrap_or_else(|| {
                sources
                    .last()
                    .expect("at least one source")
                    .files
                    .as_slice()
            })
    };
    let mut parts: BTreeMap<String, TokenStream> = loop {
        let mut parts = BTreeMap::new();
        let mut demote = Vec::new();
        for t in &vocab.objects {
            let (code, _, _) = object_struct(source_of(t), t, vocab, modules, &object_root[t]);
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
                value_class(source_of(&t), &t, vocab, modules, &object_root[&t]);
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
        let (vcode, dropped, _) = value_class(source_of(t), t, vocab, modules, &object_root[t]);
        parts.insert(t.clone(), vcode);
        say!(
            log,
            "{t}: partial mapping, emitted as read-only class {}",
            data_ident(t)
        );
        for d in dropped {
            say!(log, "{t} member not exposed: {d}");
        }
    }
    DataTypes {
        parts,
        root: object_root,
    }
}

/// The option structs as plain objects (`Config::option_objects`): every
/// field optional over `Default`, a field with no JS form supplied by a
/// provider or converted by a converter. Each from the source that defines
/// it, the primary first.
fn option_objects(
    config: &Config,
    found: &Discovery,
    vocab: &Vocab,
    bounds: &[(&'static str, u32)],
    surface: &mut Surface,
    log: &mut Log,
) {
    let mut clamped: BTreeSet<&str> = BTreeSet::new();
    for name in config.option_objects {
        let src = found.defines(name).unwrap_or(found.primary());
        let Some(obj) = option_object(&src.files, name, vocab, &src.tree.paths, &src.root, bounds)
        else {
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
        surface.put(src.root_str, obj.code);
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
}

/// Enums JS passes in, borrowed for the call: their `{ type, value }` input
/// form.
fn emit_input_enums(config: &Config, found: &Discovery, surface: &mut Surface, log: &mut Log) {
    for src in &found.sources {
        for e in input_enums(&src.files, config) {
            say!(
                log,
                "{}::{} is an input enum: {{ type, value }}",
                src.root_str,
                e.ident
            );
            surface.put(
                src.root_str,
                enum_input(e, &src.tree.paths, &src.root, config),
            );
        }
    }
}

/// The handle classes, to a fixpoint: start from the handle types the
/// wrapped impls return, generate a class for each, then follow the handles
/// *those* classes return. The type graph is finite, and `done` keeps a type
/// from being visited twice.
fn handle_classes(
    config: &Config,
    found: &Discovery,
    vocab: &Vocab,
    referenced: &Types,
    surface: &mut Surface,
    log: &mut Log,
) {
    let primary = found.primary();
    let modules = &primary.tree.paths;
    let handles_of = |set: &Types| -> BTreeSet<String> {
        set.values()
            .filter_map(|t| match vocab.classify(t) {
                Some(Js::Handle(x)) | Some(Js::HandleList(x)) => Some(x),
                _ => None,
            })
            .collect()
    };
    let mut todo: Vec<String> = handles_of(referenced).into_iter().collect();
    // Plus the handles a union carries (the toy's `Ink::Stamped(Arc<Stamp>)`):
    // the variant's object holds the class, so the class has to exist even
    // where no wrapped impl returns the type (`Stamp`).
    todo.extend(
        vocab
            .payload
            .values()
            .filter(|info| info.payload_blocker(vocab).is_none())
            .flat_map(|info| &info.variants)
            .filter_map(|(_, p)| match p {
                Payload::Value(ty) => match vocab.classify(ty) {
                    Some(Js::Handle(x)) => Some(x),
                    _ => None,
                },
                _ => None,
            }),
    );
    let mut done: BTreeSet<String> = BTreeSet::new();
    while let Some(t) = todo.pop() {
        if !done.insert(t.clone()) {
            continue;
        }
        // A key spelled through the crate that re-exports the type
        // (`<crate>::Type`) is taken relative to the defining one.
        let src = found.defines(&t).unwrap_or(primary);
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
        let (code, skipped, reached) = wrapper_class(&src.files, key, vocab, modules, &src.root);
        for s in skipped {
            say!(
                log,
                "{}::{key} method not in the namespaced class: {s}",
                src.root_str
            );
        }
        surface.put(src.root_str, code);
        todo.extend(reached.into_iter().filter(|x| !done.contains(x)));
    }
    say!(
        log,
        "wrapper classes generated: {}",
        done.iter().cloned().collect::<Vec<_>>().join(", ")
    );
}

/// The listed classes (`crate::class`), the free functions, the constants
/// the discovered and [`Role::Classes`] crates export, the error codes and the unions of
/// projections. Returns the associated constants found on the way, static
/// read-only (`crate::statics`), for [`assemble`] to hook.
fn listed_classes(
    found: &Discovery,
    extra_crates: &[ClassCrate],
    classes: &Classes,
    vocab: &Vocab,
    surface: &mut Surface,
    log: &mut Log,
) -> Vec<crate::statics::Static> {
    let mut statics = Vec::new();
    for (i, c) in classes.classes.iter().enumerate() {
        let root = classes.sources[c.source].root;
        let (code, skipped) = classes.class(i, vocab);
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
        surface.put(root, code);
        let (code, found_here, lines) = crate::statics::of_class(classes, i, vocab);
        for line in lines {
            say!(log, "{line}");
        }
        surface.put(root, code);
        statics.extend(found_here);
    }
    let scopes = found.scopes();
    let libs = scopes
        .iter()
        .map(|s| (s.root.as_str(), &s.lib))
        .chain(extra_crates.iter().map(|c| (c.root, &c.lib)));
    for (root, lib) in libs {
        let (code, skipped) = classes.functions(root, lib, vocab);
        for s in skipped {
            say!(log, "{root} function not exposed: {s}");
        }
        surface.put(root, code);
    }
    // The constants the discovered crates export, each in the namespace of
    // the module exporting it.
    for src in &found.sources {
        let (code, lines) = constants(src.root_str, &src.tree.consts, vocab);
        for line in lines {
            say!(log, "{line}");
        }
        for (root, code) in code {
            surface.put(&root, code);
        }
        // Its types' associated constants, those of a type with no class.
        let (found_here, lines) = crate::statics::of_values(
            src.root_str,
            &src.root,
            &src.parsed,
            &src.tree,
            |t| classes.lists(src.root_str, t),
            vocab,
        );
        for line in lines {
            say!(log, "{line}");
        }
        statics.extend(found_here);
    }
    // A Role::Classes crate's, as a discovered crate's (ADR 0002: every
    // public constant a wrapped crate exports is mirrored).
    for c in extra_crates {
        let (code, lines) = constants(c.root, &c.consts, vocab);
        for line in lines {
            say!(log, "{line}");
        }
        for (root, code) in code {
            surface.put(&root, code);
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
        surface.put(classes.sources[s].root, code);
    }
    for (s, name, code) in classes.data_inputs() {
        let root = classes.sources[s].root;
        say!(log, "{root}::{name} is taken by value, as {name}Object");
        surface.put(root, code);
    }
    // A `Config::non_empty` entry no input form has is upstream moving, or
    // a typo: loud, like an index bound matching nothing.
    let unused = classes.unused_non_empty();
    assert!(
        unused.is_empty(),
        "Config::non_empty: no data type taken by value has {}",
        unused.join(", ")
    );
    statics
}

/// Every settled data type, objects and read-only classes alike: the
/// registry only ever holds what a member or another data type reached.
fn emit_data_types(
    config: &Config,
    found: &Discovery,
    data: &DataTypes,
    surface: &mut Surface,
    log: &mut Log,
) {
    for (t, code) in &data.parts {
        let root = data.root[t].to_string().replace(' ', "");
        let src = found
            .sources
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
        surface.put(src.root_str, code.clone());
    }
}

/// The unions for the payload enums. Generated last, because a payload may
/// be one of the data types settled before.
fn emit_unions(
    config: &Config,
    found: &Discovery,
    classes: &Classes,
    vocab: &Vocab,
    surface: &mut Surface,
    log: &mut Log,
) {
    let sources = &found.sources;
    let modules = &found.primary().tree.paths;
    let mut unions: Vec<String> = Vec::new();
    for (name, info) in &vocab.payload {
        if let Some(why) = info.payload_blocker(vocab) {
            // Not a union, but given a JS form by another mapping: thrown
            // with its `code`, taken in its input form, or a union of
            // projections. Only an enum with no form at all is a gap.
            let claimed = classes.throws(name)
                || classes.unions.iter().any(|u| u.name == *name)
                || sources
                    .iter()
                    .any(|s| is_input(&s.files, bare(name), config));
            if !claimed {
                say!(log, "payload enum {name} not mapped: {why}");
            }
            continue;
        }
        match payload_enum_code(name, info, vocab, modules) {
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
                surface.put(root, code);
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
}

/// The surface: one namespace per source (`Config::crates`), after the
/// generic prelude, the hand-written modules and the template's own items
/// (`head`, then `tail` after everything). Writes `src/lib.rs` and the
/// `.d.ts` header, each only when it changed.
fn assemble(
    config: &Config,
    manifest: &Path,
    surface: Surface,
    shared_names: &BTreeSet<String>,
    statics: Vec<crate::statics::Static>,
    (head, tail): (TokenStream, TokenStream),
    log: &mut Log,
) {
    let parts: Vec<(&str, TokenStream)> = surface
        .0
        .into_iter()
        .map(|(root, code)| {
            let ns = config
                .namespace_of(&root)
                .unwrap_or_else(|| panic!("no Config::crates namespace for {root}"));
            (ns, code)
        })
        .collect();
    let surfaces = namespaced(&parts, shared_names);
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
        let (alias, mut dts_header) = crate::dispose::aliases(&whole);
        // The associated constants: getters and objects set when the module
        // loads, declared in the same header.
        let (statics, statics_dts, lines) = crate::statics::hook(&whole, statics);
        for line in lines {
            say!(log, "{line}");
        }
        dts_header.push_str(&statics_dts);
        write_if_changed(&manifest.join(DTS_HEADER), &dts_header);
        let inspect = inspect_members(&whole);
        // napi runs one `module_exports` hook: this one, calling the rest.
        let define = (!statics.is_empty()).then(|| quote!(define_statics(&env, exports)?;));
        quote! {
            #file
            #alias
            #statics
            #[doc = " What the module sets when it loads, after napi registers its items."]
            #[napi(module_exports)]
            pub fn on_load(env: Env, exports: Object) -> Result<()> {
                alias_dispose(&env, exports)?;
                #define
                Ok(())
            }
            #inspect
        }
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
}

/// The option objects no entry needs to list: each struct with `Default`
/// and no type parameter that a listed member of a discovered crate -- a
/// method of a listed or projected class, a listed function -- takes by `&`,
/// defined beside it and not a class itself (ADR 0001: what the allowlisted
/// members reach). `Counter::new(&Options)` makes `Options` one.
fn derived_option_objects(classes: &Classes, scopes: &[&Scope]) -> BTreeSet<String> {
    let config = &classes.config;
    let mut out = BTreeSet::new();
    for scope in scopes {
        let methods = classes
            .classes
            .iter()
            .filter(|c| classes.sources[c.source].root == scope.root)
            .flat_map(|c| inherent_impls(&scope.files, &c.name))
            .flat_map(|imp| &imp.items)
            .filter_map(|it| match it {
                syn::ImplItem::Fn(f) if is_pub(&f.vis) => Some(&f.sig),
                _ => None,
            });
        let functions = scope
            .lib
            .iter()
            .flat_map(|f| &f.items)
            .filter_map(|i| match i {
                Item::Fn(f)
                    if is_pub(&f.vis)
                        && config
                            .functions
                            .contains(&(scope.root.as_str(), f.sig.ident.to_string().as_str())) =>
                {
                    Some(&f.sig)
                }
                _ => None,
            });
        for sig in methods.chain(functions) {
            for arg in &sig.inputs {
                let syn::FnArg::Typed(a) = arg else { continue };
                let Shape::Ref {
                    mutable: false,
                    inner,
                    ..
                } = Shape::of(&a.ty)
                else {
                    continue;
                };
                let Some(name) = Shape::of(inner).last().map(str::to_string) else {
                    continue;
                };
                let option = !classes.classes.iter().any(|c| c.name == name)
                    && !config.node_types.contains(&name.as_str())
                    && crate::class::find_struct(&scope.files, &name)
                        .is_some_and(|st| st.generics.type_params().next().is_none())
                    && has_default(&scope.files, &name);
                if option {
                    out.insert(name);
                }
            }
        }
    }
    out
}

/// The `Copy` structs the classes of a [`Role::Classes`] crate take or return
/// (by value, by `&`, in an `Option` or a `Result`) that no class covers yet,
/// each with its crate's root and the first member reaching it
/// (`toy_dep::Grid::bounds`). Only what the crate exports at its root path,
/// with no generic parameter and a public inherent function. A slice or a
/// list of one is not followed: an array of instances is a poor form for
/// what is usually a buffer.
fn reached_values(classes: &Classes, crates: &[ClassCrate]) -> Vec<(&'static str, String, String)> {
    let mut out: Vec<(&'static str, String, String)> = Vec::new();
    for krate in crates {
        let of_crate = classes
            .classes
            .iter()
            .filter(|c| classes.sources[c.source].root == krate.root);
        for c in of_crate {
            let sigs = inherent_impls(&krate.files, &c.name)
                .flat_map(|imp| &imp.items)
                .filter_map(|it| match it {
                    syn::ImplItem::Fn(f) if is_pub(&f.vis) => Some(&f.sig),
                    _ => None,
                });
            for sig in sigs {
                let args = sig.inputs.iter().filter_map(|a| match a {
                    syn::FnArg::Typed(t) => Some(&*t.ty),
                    syn::FnArg::Receiver(_) => None,
                });
                let ret = match &sig.output {
                    syn::ReturnType::Type(_, t) => Some(&**t),
                    syn::ReturnType::Default => None,
                };
                for ty in args.chain(ret) {
                    let Some(name) = value_named(ty) else {
                        continue;
                    };
                    let value = !classes.lists(krate.root, &name)
                        && !out.iter().any(|(r, n, _)| *r == krate.root && *n == name)
                        && krate.exported.contains(&name)
                        && crate::class::find_struct(&krate.files, &name)
                            .is_some_and(|st| st.generics.params.is_empty())
                        && derives(&krate.files, &name, "Copy")
                        // Something to call: a class of no member is an
                        // opaque token JS can neither build nor read.
                        && inherent_impls(&krate.files, &name)
                            .flat_map(|imp| &imp.items)
                            .any(|it| matches!(it, syn::ImplItem::Fn(f) if is_pub(&f.vis)));
                    if value {
                        let by = format!("{}::{}::{}", krate.root, c.name, sig.ident);
                        out.push((krate.root, name, by));
                    }
                }
            }
        }
    }
    out
}

/// The type a signature names, as a bare name, under `&`, `Option` and
/// `Result`'s `Ok`: `Option<&Ty>` is `Ty`. A path qualified by anything but
/// `crate` or `self` is another crate's type, and not one.
fn value_named(ty: &syn::Type) -> Option<String> {
    match Shape::of(ty) {
        Shape::Ref { inner, .. } | Shape::Option(inner) | Shape::Result { ok: inner, .. } => {
            value_named(inner)
        }
        Shape::Path { segs, args } if args.is_empty() => match segs.as_slice() {
            [name] => Some(name.clone()),
            [q, name] if q == "crate" || q == "self" => Some(name.clone()),
            _ => None,
        }
        .filter(|n| n != "Self"),
        _ => None,
    }
}

/// The names a crate root re-exports by name (`pub use dep::{A, B as C}`
/// gives `A` and `C`). ponytail: a glob (`pub use dep::*`) gives nothing; no
/// crate read here re-exports that way.
fn reexported_names(lib: &syn::File) -> Vec<String> {
    fn leaves(t: &syn::UseTree, out: &mut Vec<String>) {
        match t {
            syn::UseTree::Path(p) => leaves(&p.tree, out),
            syn::UseTree::Name(n) => out.push(n.ident.to_string()),
            syn::UseTree::Rename(r) => out.push(r.rename.to_string()),
            syn::UseTree::Glob(_) => {}
            syn::UseTree::Group(g) => g.items.iter().for_each(|i| leaves(i, out)),
        }
    }
    let mut out = Vec::new();
    for item in &lib.items {
        if let Item::Use(u) = item
            && is_pub(&u.vis)
        {
            leaves(&u.tree, &mut out);
        }
    }
    out
}

/// The generated file's header after its first line.
const HEADER_REST: &str = concat!(
    "// Regenerate with `touch build.rs && cargo build`.\n",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_named_through_refs_options_and_results() {
        let named = |t: &str| value_named(&syn::parse_str(t).unwrap());
        assert_eq!(named("Area").as_deref(), Some("Area"));
        assert_eq!(named("Option<&crate::Area>").as_deref(), Some("Area"));
        assert_eq!(named("Result<Area, Error>").as_deref(), Some("Area"));
        // Another crate's, a generic, a slice, a list, `Self`: none.
        for t in ["other::Area", "Cell<u8>", "&[Area]", "Vec<Area>", "Self"] {
            assert_eq!(named(t), None, "{t}");
        }
    }

    #[test]
    fn a_root_reexports_by_name() {
        let lib: syn::File =
            syn::parse_str("pub use dep::{A, b::B as C}; pub use dep::*; use dep::D;").unwrap();
        assert_eq!(reexported_names(&lib), ["A", "C"]);
    }
}
