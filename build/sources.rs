//! Finding and parsing the upstream sources.
//!
//! Phase 1. Locate the crates the resolver actually picked -- an env override,
//! then a git submodule, then the cargo registry at the version Cargo.lock
//! names -- read them off disk, and hand back `syn` trees.
//!
//! Nothing here knows anything about napi or JavaScript.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use syn::{Item, Visibility};

/// Exact versions picked by the resolver, straight out of our own Cargo.lock.
/// Cheap hand-rolled scan: no serde/toml build-dependency needed.
pub fn locked_versions() -> BTreeMap<String, String> {
    let lock = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("Cargo.lock");
    println!("cargo::rerun-if-changed={}", lock.display());
    let mut out = BTreeMap::new();
    // Without it every crate would fall through to whatever the registry
    // cache holds, and the bindings would describe some other version.
    let text = fs::read_to_string(&lock)
        .unwrap_or_else(|e| panic!("{}: {e}; run `cargo generate-lockfile`", lock.display()));
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("version = ")
            && let Some(n) = name.take()
        {
            // Keep the highest version if a crate appears twice in the graph.
            let v = v.trim_matches('"').to_string();
            let better = out
                .get(&n)
                .is_none_or(|old: &String| semver_key(&v) > semver_key(old));
            if better {
                out.insert(n, v);
            }
        }
    }
    out
}

pub fn semver_key(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

pub fn cargo_home() -> PathBuf {
    env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").expect("HOME unset")).join(".cargo"))
}

/// `$CARGO_HOME/registry/src/<any-index>/<pkg>-<version>/src`
pub fn registry_src(pkg: &str, want: &str) -> Option<PathBuf> {
    for index in fs::read_dir(cargo_home().join("registry/src"))
        .ok()?
        .flatten()
    {
        let Ok(entries) = fs::read_dir(index.path()) else {
            continue;
        };
        for e in entries.flatten() {
            if e.file_name().to_string_lossy() == format!("{pkg}-{want}") {
                return Some(e.path().join("src"));
            }
        }
    }
    None
}

/// Resolution order, most explicit first:
///   1. `<PKG>_SRC_DIR` env var (vendored checkout / git submodule anywhere)
///   2. `vendor/resvg/crates/<pkg>/src` (the upstream monorepo as a submodule)
///   3. cargo registry cache, exact locked version
///
/// A vendored checkout must declare the locked version. There is no
/// "newest version present" fallback: a registry cache shared with other
/// projects would hand over sources the resolver never picked.
pub fn locate(pkg: &str, marker: &str, locked: &BTreeMap<String, String>) -> PathBuf {
    let key = format!("{}_SRC_DIR", pkg.to_uppercase().replace('-', "_"));
    println!("cargo::rerun-if-env-changed={key}");
    if let Some(dir) = env::var_os(&key) {
        let p = PathBuf::from(dir);
        assert!(
            p.join(marker).exists(),
            "{key} points at {}, but {marker} is missing there",
            p.display()
        );
        return p;
    }

    let vendored = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("vendor/resvg/crates")
        .join(pkg)
        .join("src");
    let locked_ver = locked
        .get(pkg)
        .unwrap_or_else(|| panic!("`{pkg}` is not in Cargo.lock"));
    if vendored.join(marker).exists() {
        // `version.workspace = true` has no literal to compare; trust it.
        let manifest = vendored.with_file_name("Cargo.toml");
        let declared = fs::read_to_string(&manifest).ok().and_then(|t| {
            t.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("version = ")
                    .map(|v| v.trim_matches('"').to_string())
            })
        });
        if let Some(v) = declared {
            assert!(
                &v == locked_ver,
                "{} declares {pkg} {v}, but Cargo.lock picked {locked_ver}",
                manifest.display()
            );
        }
        return vendored;
    }

    registry_src(pkg, locked_ver)
        .filter(|p| p.join(marker).exists())
        .unwrap_or_else(|| {
            panic!(
                "cannot find the sources of `{pkg}` {locked_ver} (looked for {marker}).\n\
                 Run `cargo fetch` first, or point {key} at a checkout."
            )
        })
}

pub fn parse(path: &Path) -> syn::File {
    println!("cargo::rerun-if-changed={}", path.display());
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: parse error: {e}", path.display()))
}

pub fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// Parses every `.rs` under a crate's `src`, for crates we only mine for
/// vocabulary (newtypes, aliases) rather than wrap.
/// Drops the files a crate keeps to itself.
///
/// A `mod x;` without `pub` is an implementation detail, and what it declares
/// is not reachable by the crate's own path. fontdb vendors a copy of
/// ttf-parser under one: its `pub enum Style` is not `usvg::fontdb::Style`, and
/// mapping it emitted `Style` twice and referenced `usvg::fontdb::Width`, a
/// type that does not exist. Anything the crate wants public it re-exports, and
/// the re-export is what the root file shows.
/// What the crate root re-exports, grouped by the module it comes from, and
/// under the name the root gives it: `pub use ttf_parser::Width as Stretch`
/// answers `ttf_parser -> [(Width, Stretch)]`.
fn reexports(root: &syn::File) -> BTreeMap<String, Vec<(String, String)>> {
    fn walk(
        tree: &syn::UseTree,
        path: &mut Vec<String>,
        out: &mut BTreeMap<String, Vec<(String, String)>>,
    ) {
        match tree {
            syn::UseTree::Path(p) => {
                path.push(p.ident.to_string());
                walk(&p.tree, path, out);
                path.pop();
            }
            // `self` and `crate` are where the path starts, not a module it
            // names; the module is whatever follows.
            syn::UseTree::Name(n) => {
                if let Some(m) = path.iter().find(|s| *s != "self" && *s != "crate") {
                    out.entry(m.clone())
                        .or_default()
                        .push((n.ident.to_string(), n.ident.to_string()));
                }
            }
            syn::UseTree::Rename(r) => {
                if let Some(m) = path.iter().find(|s| *s != "self" && *s != "crate") {
                    out.entry(m.clone())
                        .or_default()
                        .push((r.ident.to_string(), r.rename.to_string()));
                }
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    walk(t, path, out);
                }
            }
            // A glob says nothing about which names it carries.
            syn::UseTree::Glob(_) => {}
        }
    }
    let mut out = BTreeMap::new();
    for item in &root.items {
        if let Item::Use(u) = item
            && matches!(u.vis, Visibility::Public(_))
        {
            walk(&u.tree, &mut Vec::new(), &mut out);
        }
    }
    out
}

/// Renames a kept item to the name the crate root exports it under, so what is
/// parsed downstream is what a caller of the crate can actually write.
fn rename(item: &mut Item, from: &str, to: &str) {
    let to_ident = |i: &mut proc_macro2::Ident| {
        if *i == from {
            *i = proc_macro2::Ident::new(to, i.span());
        }
    };
    match item {
        Item::Struct(st) => to_ident(&mut st.ident),
        Item::Enum(e) => to_ident(&mut e.ident),
        Item::Type(t) => to_ident(&mut t.ident),
        Item::Impl(im) => {
            if let syn::Type::Path(p) = &mut *im.self_ty
                && let Some(seg) = p.path.segments.last_mut()
            {
                to_ident(&mut seg.ident);
            }
        }
        _ => {}
    }
}

/// The name an item declares, when it declares one this generator cares about.
fn declared(item: &Item) -> Option<String> {
    Some(match item {
        Item::Struct(st) => st.ident.to_string(),
        Item::Enum(e) => e.ident.to_string(),
        Item::Type(t) => t.ident.to_string(),
        Item::Impl(im) => match &*im.self_ty {
            syn::Type::Path(p) => p.path.segments.last()?.ident.to_string(),
            _ => return None,
        },
        _ => return None,
    })
}

pub fn public_only(dir: &Path, files: Vec<(PathBuf, syn::File)>) -> Vec<(PathBuf, syn::File)> {
    let root = files.iter().find(|(p, _)| {
        p.file_name().and_then(|n| n.to_str()) == Some("lib.rs") && p.parent() == Some(dir)
    });
    let Some((_, root)) = root else { return files };
    let private: BTreeSet<String> = root
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Mod(m) if !is_pub(&m.vis) => Some(m.ident.to_string()),
            _ => None,
        })
        .collect();
    if private.is_empty() {
        return files;
    }
    let exported = reexports(root);
    files
        .into_iter()
        .filter_map(|(path, mut file)| {
            let rel = path.strip_prefix(dir).unwrap_or(&path);
            let module = rel
                .components()
                .next()
                .map(|c| {
                    c.as_os_str()
                        .to_str()
                        .unwrap_or_default()
                        .trim_end_matches(".rs")
                })
                .unwrap_or_default()
                .to_string();
            if !private.contains(&module) {
                return Some((path, file));
            }
            // A private module is not all-or-nothing: the crate root may lift
            // individual names out of it, and those names are public API --
            // `pub use ttf_parser::Width as Stretch` is how a caller writes
            // `fontdb::Stretch`. Keep exactly those, under the name the root
            // gives them, and drop the rest of the module. Taking the whole
            // file instead is the mistake this function was written to stop:
            // 2857 lines of vendored parser whose `Style` is not fontdb's.
            let lifted = exported.get(&module)?;
            file.items.retain_mut(|item| {
                let Some(name) = declared(item) else {
                    return false;
                };
                let Some((from, to)) = lifted.iter().find(|(from, _)| *from == name) else {
                    return false;
                };
                rename(item, from, to);
                true
            });
            (!file.items.is_empty()).then_some((path, file))
        })
        .collect()
}

pub fn parse_crate(dir: &Path) -> Vec<(PathBuf, syn::File)> {
    pub fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let f = parse(&p);
            (p, f)
        })
        .collect()
}

/// A crate's module tree, read the way rustc reads it: from `lib.rs`, through
/// `mod x;` (to `x.rs` or `x/mod.rs`, or `#[path]`), with `pub use` resolved --
/// globs, names, renames, `self::`/`crate::`/`super::`. A type's public module
/// is where an export puts it, not the stem of the file it is written in: the
/// stem confused `parser/filter.rs` with `tree/filter.rs`, made every `mod.rs`
/// a crate root, and took `parser/svgtree/mod.rs`'s private `SvgNode` for a
/// public type.
///
/// ponytail: file modules only. An inline `mod x { .. }` is not followed; no
/// crate parsed here exports through one. Re-exports of other crates
/// (`pub use fontdb;`) are left out, as before.
pub struct ModTree {
    /// Public type name -> the namespace it is reachable at (`""` the root,
    /// `"filter"`). Root wins when a type is reachable at both.
    pub paths: BTreeMap<String, String>,
    /// File -> the namespace its exported types land in. A file none of whose
    /// types is exported is absent.
    pub file_ns: BTreeMap<PathBuf, String>,
    /// (file, type name) for every `pub` type no export reaches.
    pub private: BTreeSet<(PathBuf, String)>,
}

impl ModTree {
    pub fn load(src: &Path, parsed: &[(PathBuf, syn::File)]) -> ModTree {
        let files: BTreeMap<&Path, &syn::File> =
            parsed.iter().map(|(p, f)| (p.as_path(), f)).collect();
        // Module path -> its file, found by following `mod x;` from lib.rs.
        let mut modules: BTreeMap<Vec<String>, PathBuf> = BTreeMap::new();
        let mut stack = vec![(Vec::<String>::new(), src.join("lib.rs"))];
        while let Some((path, file)) = stack.pop() {
            let Some(f) = files.get(file.as_path()) else {
                continue;
            };
            // Children of `lib.rs`/`mod.rs` sit beside it; of `x.rs`, in `x/`.
            let stem = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let dir = if matches!(stem, "lib" | "mod") {
                file.parent().unwrap().to_path_buf()
            } else {
                file.with_extension("")
            };
            for item in &f.items {
                let Item::Mod(m) = item else { continue };
                if m.content.is_some() {
                    continue;
                }
                let name = m.ident.to_string();
                let explicit = m
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("path"))
                    .and_then(|a| match &a.meta {
                        syn::Meta::NameValue(nv) => match &nv.value {
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(s),
                                ..
                            }) => Some(s.value()),
                            _ => None,
                        },
                        _ => None,
                    });
                let child = match explicit {
                    Some(p) => file.parent().unwrap().join(p),
                    None if dir.join(format!("{name}.rs")).exists() => {
                        dir.join(format!("{name}.rs"))
                    }
                    None => dir.join(&name).join("mod.rs"),
                };
                let mut p = path.clone();
                p.push(name);
                stack.push((p.clone(), child.clone()));
                modules.insert(p, child);
            }
        }
        modules.insert(Vec::new(), src.join("lib.rs"));

        // What each module exports: (public name, target). A target is a type
        // defined in a module, or a module.
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
        enum Target {
            Type(Vec<String>, String),
            Module(Vec<String>),
        }
        fn exports(
            m: &[String],
            modules: &BTreeMap<Vec<String>, PathBuf>,
            files: &BTreeMap<&Path, &syn::File>,
            depth: usize,
        ) -> Vec<(String, Target)> {
            assert!(depth < 32, "re-export cycle through {m:?}");
            let Some(f) = modules.get(m).and_then(|p| files.get(p.as_path())) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            // A path in a `use`, made absolute; None when it leaves the crate.
            let resolve = |segs: &[String]| -> Option<Vec<String>> {
                let mut at: Vec<String> = m.to_vec();
                let mut rest = segs;
                match rest.first().map(String::as_str) {
                    Some("crate") => {
                        at.clear();
                        rest = &rest[1..];
                    }
                    Some("self") => rest = &rest[1..],
                    _ => {}
                }
                while rest.first().map(String::as_str) == Some("super") {
                    at.pop();
                    rest = &rest[1..];
                }
                at.extend(rest.iter().cloned());
                modules.contains_key(&at).then_some(at)
            };
            /// One `use` leaf: its path, and `(name, as)` -- or None for a glob.
            type Leaf = (Vec<String>, Option<(String, String)>);
            fn walk(t: &syn::UseTree, path: &mut Vec<String>, out: &mut Vec<Leaf>) {
                match t {
                    syn::UseTree::Path(p) => {
                        path.push(p.ident.to_string());
                        walk(&p.tree, path, out);
                        path.pop();
                    }
                    syn::UseTree::Name(n) => out.push((
                        path.clone(),
                        Some((n.ident.to_string(), n.ident.to_string())),
                    )),
                    syn::UseTree::Rename(r) => out.push((
                        path.clone(),
                        Some((r.ident.to_string(), r.rename.to_string())),
                    )),
                    syn::UseTree::Glob(_) => out.push((path.clone(), None)),
                    syn::UseTree::Group(g) => g.items.iter().for_each(|i| walk(i, path, out)),
                }
            }
            for item in &f.items {
                match item {
                    Item::Struct(s) if is_pub(&s.vis) => out.push((
                        s.ident.to_string(),
                        Target::Type(m.to_vec(), s.ident.to_string()),
                    )),
                    Item::Enum(e) if is_pub(&e.vis) => out.push((
                        e.ident.to_string(),
                        Target::Type(m.to_vec(), e.ident.to_string()),
                    )),
                    Item::Type(t) if is_pub(&t.vis) => out.push((
                        t.ident.to_string(),
                        Target::Type(m.to_vec(), t.ident.to_string()),
                    )),
                    Item::Mod(md) if is_pub(&md.vis) && md.content.is_none() => {
                        let mut p = m.to_vec();
                        p.push(md.ident.to_string());
                        out.push((md.ident.to_string(), Target::Module(p)));
                    }
                    Item::Use(u) if is_pub(&u.vis) => {
                        let mut leaves = Vec::new();
                        walk(&u.tree, &mut Vec::new(), &mut leaves);
                        for (path, leaf) in leaves {
                            match leaf {
                                // `pub use x::*`: everything x exports.
                                None => {
                                    if let Some(target) = resolve(&path) {
                                        out.extend(exports(&target, modules, files, depth + 1));
                                    }
                                }
                                // `pub use x::N as K`: x's N, as K -- or x::N itself
                                // when N names a module.
                                Some((n, k)) => {
                                    let Some(target) = resolve(&path) else {
                                        continue;
                                    };
                                    // `pub use other_crate;` or `pub use self::Item`:
                                    // nothing new to read in this module itself.
                                    if target == m {
                                        let mut as_module = target.clone();
                                        as_module.push(n.clone());
                                        if modules.contains_key(&as_module) {
                                            out.push((k, Target::Module(as_module)));
                                        }
                                        continue;
                                    }
                                    let mut as_module = target.clone();
                                    as_module.push(n.clone());
                                    if modules.contains_key(&as_module) {
                                        out.push((k, Target::Module(as_module)));
                                    } else if let Some((_, t)) =
                                        exports(&target, modules, files, depth + 1)
                                            .into_iter()
                                            .find(|(e, _)| *e == n)
                                    {
                                        out.push((k, t));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            out
        }

        // Every public namespace, from the root down through exported modules.
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        let mut reached: BTreeMap<(Vec<String>, String), String> = BTreeMap::new();
        let mut todo = vec![(String::new(), Vec::<String>::new())];
        let mut seen = BTreeSet::new();
        while let Some((ns, module)) = todo.pop() {
            if !seen.insert(module.clone()) {
                continue;
            }
            for (name, target) in exports(&module, &modules, &files, 0) {
                match target {
                    Target::Type(def, ty) => {
                        // The shortest namespace wins, so the root does:
                        // `usvg::Image` is what `Image` means.
                        let better = |old: &String| old.len() > ns.len();
                        if paths.get(&name).is_none_or(better) {
                            paths.insert(name.clone(), ns.clone());
                        }
                        let key = (def, ty);
                        if reached.get(&key).is_none_or(better) {
                            reached.insert(key, ns.clone());
                        }
                    }
                    Target::Module(m) => {
                        let sub = if ns.is_empty() {
                            name
                        } else {
                            format!("{ns}::{name}")
                        };
                        todo.push((sub, m));
                    }
                }
            }
        }

        let mut file_ns = BTreeMap::new();
        let mut private = BTreeSet::new();
        for (module, file) in &modules {
            let Some(f) = files.get(file.as_path()) else {
                continue;
            };
            for item in &f.items {
                let name = match item {
                    Item::Struct(s) if is_pub(&s.vis) => s.ident.to_string(),
                    Item::Enum(e) if is_pub(&e.vis) => e.ident.to_string(),
                    Item::Type(t) if is_pub(&t.vis) => t.ident.to_string(),
                    _ => continue,
                };
                match reached.get(&(module.clone(), name.clone())) {
                    Some(ns) => {
                        let prev = file_ns.insert(file.clone(), ns.clone());
                        assert!(
                            prev.is_none_or(|p| p == *ns),
                            "{}: types exported into two namespaces",
                            file.display()
                        );
                    }
                    None => {
                        private.insert((file.clone(), name));
                    }
                }
            }
        }
        ModTree {
            paths,
            file_ns,
            private,
        }
    }

    /// The files of the crate as the generator should see them: every file
    /// the module tree reaches (a binary's `main.rs` is not one), with the
    /// `pub` types no export reaches taken out. Their `impl`s, functions and
    /// statics stay: `Tree::from_data` is implemented in the private `parser`,
    /// and `POW_VEC` lives in the private `writer`.
    pub fn public_view(
        &self,
        parsed: &[(PathBuf, syn::File)],
        src: &Path,
    ) -> Vec<(PathBuf, syn::File)> {
        parsed
            .iter()
            .filter(|(p, _)| {
                p.file_name().and_then(|n| n.to_str()) != Some("main.rs") || p.parent() != Some(src)
            })
            .map(|(p, f)| {
                let mut f = f.clone();
                f.items.retain(|item| {
                    let name = match item {
                        Item::Struct(s) => s.ident.to_string(),
                        Item::Enum(e) => e.ident.to_string(),
                        Item::Type(t) => t.ident.to_string(),
                        _ => return true,
                    };
                    !self.private.contains(&(p.clone(), name))
                });
                (p.clone(), f)
            })
            .collect()
    }
}

/// `ModTree` on a crate small enough to read: a glob through a private module,
/// a public submodule, two `filter.rs` in different places, a rename, and pub
/// types no export reaches. Run by `build.rs` on every build.
pub fn modtree_selftest(scratch: &Path) {
    let root = scratch.join("modtree-selftest");
    let _ = fs::remove_dir_all(&root);
    let files = [
        (
            "lib.rs",
            "mod tree; mod hidden; mod parser; pub use tree::*; pub use hidden::Lifted as Renamed;",
        ),
        (
            "tree/mod.rs",
            "pub mod filter; mod geom; pub use self::geom::*; pub use tiny; pub struct Image;",
        ),
        ("tree/filter.rs", "pub struct Image; pub struct Blur;"),
        ("tree/geom.rs", "pub struct Rect;"),
        ("hidden.rs", "pub struct Lifted; pub struct Secret;"),
        ("parser/mod.rs", "mod filter;"),
        ("parser/filter.rs", "pub struct Internal;"),
        ("main.rs", "pub struct Bin;"),
    ];
    let mut parsed = Vec::new();
    for (rel, text) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        parsed.push((p, syn::parse_file(text).unwrap()));
    }
    let t = ModTree::load(&root, &parsed);
    let at = |n: &str| t.paths.get(n).map(String::as_str);
    assert_eq!(at("Image"), Some(""), "the root wins over filter::Image");
    assert_eq!(at("Blur"), Some("filter"));
    assert_eq!(at("Rect"), Some(""), "a glob through a private module");
    assert_eq!(at("Renamed"), Some(""), "a rename");
    assert_eq!(at("Secret"), None);
    assert_eq!(
        at("Internal"),
        None,
        "parser/filter.rs is not tree/filter.rs"
    );
    assert_eq!(
        t.file_ns
            .get(&root.join("tree/filter.rs"))
            .map(String::as_str),
        Some("filter")
    );
    assert!(!t.file_ns.contains_key(&root.join("parser/filter.rs")));
    assert!(
        t.private
            .contains(&(root.join("hidden.rs"), "Secret".into()))
    );
    let view = t.public_view(&parsed, &root);
    assert!(
        view.iter().all(|(p, _)| !p.ends_with("main.rs")),
        "a binary is not the library"
    );
    let _ = fs::remove_dir_all(&root);
}
