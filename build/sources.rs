//! Finding and parsing the upstream sources.
//!
//! Phase 1. Locate the crates the resolver actually picked -- an env override,
//! then the cargo registry at the version Cargo.lock names -- read them off
//! disk, and hand back `syn` trees.
//!
//! Nothing here knows anything about napi or JavaScript.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use syn::{Ident, Item, ItemImpl, Visibility};

/// Exact versions picked by the resolver, straight out of our own Cargo.lock.
/// Cheap hand-rolled scan: no serde/toml build-dependency needed. A crate in
/// the graph twice keeps every version, and `locate` refuses to pick one.
pub fn locked_versions() -> BTreeMap<String, Vec<String>> {
    let lock = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("Cargo.lock");
    println!("cargo::rerun-if-changed={}", lock.display());
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
            out.entry(n)
                .or_default()
                .push(v.trim_matches('"').to_string());
        }
    }
    out
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
///   2. cargo registry cache, exact locked version
///
/// There is no "newest version present" fallback: a registry cache shared
/// with other projects would hand over sources the resolver never picked.
pub fn locate(pkg: &str, marker: &str, locked: &BTreeMap<String, Vec<String>>) -> PathBuf {
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

    let locked_ver = match locked.get(pkg).map(Vec::as_slice) {
        Some([v]) => v,
        Some(vs) => panic!("`{pkg}` is in Cargo.lock twice ({vs:?}): say which one to read"),
        None => panic!("`{pkg}` is not in Cargo.lock"),
    };
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

/// Every inherent impl of `ty`, named by its last path segment.
pub fn inherent_impls<'a>(
    files: &'a [syn::File],
    ty: &'a str,
) -> impl Iterator<Item = &'a ItemImpl> {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(move |i| match i {
            Item::Impl(imp)
                if imp.trait_.is_none() && self_ident(imp).is_some_and(|id| id == ty) =>
            {
                Some(imp)
            }
            _ => None,
        })
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

/// The name a `pub` struct, enum or type alias declares.
pub fn item_ident(item: &Item) -> Option<&Ident> {
    match item {
        Item::Struct(s) if is_pub(&s.vis) => Some(&s.ident),
        Item::Enum(e) if is_pub(&e.vis) => Some(&e.ident),
        Item::Type(t) if is_pub(&t.vis) => Some(&t.ident),
        _ => None,
    }
}

/// The type an impl block is for, by its last path segment: upstream writes
/// `impl crate::Tree` as well as `impl Tree`.
pub fn self_ident(imp: &ItemImpl) -> Option<&Ident> {
    match &*imp.self_ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| &s.ident),
        _ => None,
    }
}

/// The name an item declares, when it declares one this generator cares about.
fn declared(item: &Item) -> Option<String> {
    match item {
        Item::Impl(im) => self_ident(im),
        _ => item_ident(item),
    }
    .map(ToString::to_string)
}

/// Every `.rs` under a crate's `src`, parsed. `ModTree` then decides which of
/// them, and which of their items, are the crate's public API.
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
/// `mod x;` (to `x.rs` or `x/mod.rs`), with `pub use` resolved --
/// globs, names, renames, `self::`/`crate::`/`super::`. A type's public module
/// is where an export puts it, not the stem of the file it is written in: the
/// stem confused `parser/filter.rs` with `tree/filter.rs`, made every `mod.rs`
/// a crate root, and took `parser/svgtree/mod.rs`'s private `SvgNode` for a
/// public type.
///
/// ponytail: file modules only. An inline `mod x { .. }` is not followed, and
/// neither is `#[path]`; no crate parsed here uses either. Re-exports of other crates
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
    /// (file, type name) -> the name an export gives it, when it differs:
    /// fontdb's `pub use ttf_parser::Width as Stretch`.
    pub renamed: BTreeMap<(PathBuf, String), String>,
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
                let child = if dir.join(format!("{name}.rs")).exists() {
                    dir.join(format!("{name}.rs"))
                } else {
                    dir.join(&name).join("mod.rs")
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
                    _ => {
                        if let Some(id) = item_ident(item) {
                            out.push((id.to_string(), Target::Type(m.to_vec(), id.to_string())));
                        }
                    }
                }
            }
            out
        }

        // Every public namespace, from the root down through exported modules.
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        let mut reached: BTreeMap<(Vec<String>, String), String> = BTreeMap::new();
        let mut renames: BTreeMap<(Vec<String>, String), String> = BTreeMap::new();
        let mut todo = vec![(String::new(), Vec::<String>::new())];
        let mut seen = BTreeSet::new();
        while let Some((ns, module)) = todo.pop() {
            if !seen.insert(module.clone()) {
                continue;
            }
            for (name, target) in exports(&module, &modules, &files, 0) {
                match target {
                    Target::Type(def, ty) => {
                        if name != ty {
                            renames.insert((def.clone(), ty.clone()), name.clone());
                        }
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
        let mut renamed = BTreeMap::new();
        for (module, file) in &modules {
            let Some(f) = files.get(file.as_path()) else {
                continue;
            };
            for item in &f.items {
                let Some(name) = item_ident(item).map(ToString::to_string) else {
                    continue;
                };
                if let Some(to) = renames.get(&(module.clone(), name.clone())) {
                    renamed.insert((file.clone(), name.clone()), to.clone());
                }
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
            renamed,
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
        // An `impl` of a type no export reaches is no API either, and its
        // signatures would otherwise make what they mention look wanted:
        // ttf_parser's `Name::language()` pulled fontdb's `Language` in.
        let hidden: BTreeSet<&str> = self
            .private
            .iter()
            .map(|(_, n)| n.as_str())
            .filter(|n| !self.paths.contains_key(*n))
            .collect();
        parsed
            .iter()
            .filter(|(p, _)| {
                p.file_name().and_then(|n| n.to_str()) != Some("main.rs") || p.parent() != Some(src)
            })
            .map(|(p, f)| {
                let mut f = f.clone();
                // Only `pub` types are ever private-listed, so a type that
                // is not `pub` stays as well.
                f.items.retain(|item| match item {
                    Item::Impl(_) => declared(item).is_none_or(|n| !hidden.contains(n.as_str())),
                    _ => item_ident(item)
                        .is_none_or(|n| !self.private.contains(&(p.clone(), n.to_string()))),
                });
                // Under the name the export gives it, so what is parsed is what
                // a caller of the crate can write: `Stretch`, not `Width`.
                for item in &mut f.items {
                    if let Some(name) = declared(item)
                        && let Some(to) = self.renamed.get(&(p.clone(), name.clone()))
                    {
                        rename(item, &name, to);
                    }
                }
                (p.clone(), f)
            })
            .collect()
    }
}
