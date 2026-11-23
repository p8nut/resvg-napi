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

/// The Cargo.lock the building crate resolves against: its own, or the
/// workspace's above it when it is a member. The nearest one walking up from
/// `CARGO_MANIFEST_DIR`, which is the one cargo uses.
pub fn lockfile() -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    manifest
        .ancestors()
        .map(|d| d.join("Cargo.lock"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| {
            panic!(
                "no Cargo.lock in {} or above it; run `cargo generate-lockfile`",
                manifest.display()
            )
        })
}

/// Exact versions picked by the resolver, straight out of the Cargo.lock.
/// Cheap hand-rolled scan: no serde/toml build-dependency needed. A crate in
/// the graph twice keeps every version, and `locate` refuses to pick one.
pub fn locked_versions() -> BTreeMap<String, Vec<String>> {
    let lock = lockfile();
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
        .unwrap_or_else(|| env::home_dir().expect("no home directory").join(".cargo"))
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

/// The `src` directory of the workspace member named `pkg`, if the workspace
/// of [`lockfile`] has one: what Cargo resolves a path dependency to. Read
/// off the workspace's `members` and each member's `[package] name`, by the
/// same cheap scan as [`locked_versions`].
///
/// ponytail: listed members only, no globs (`crates/*`), no `package.name`
/// inherited from the workspace.
pub fn workspace_member(pkg: &str) -> Option<PathBuf> {
    let lock = lockfile();
    let root = lock.parent()?;
    let manifest = fs::read_to_string(root.join("Cargo.toml")).ok()?;
    // `members = ["a", "b"]`, on one line or several, starting a line.
    let members = manifest
        .split_once("\nmembers")
        .and_then(|(_, rest)| rest.split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list)
        .unwrap_or_default();
    let name_of = |dir: &Path| {
        let text = fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        text.lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix("name = "))
            .map(|v| v.trim_matches('"').to_string())
    };
    std::iter::once(root.to_path_buf())
        .chain(
            members
                .split(',')
                .map(|m| m.trim().trim_matches('"'))
                .filter(|m| !m.is_empty())
                .map(|m| root.join(m)),
        )
        .find(|dir| name_of(dir).as_deref() == Some(pkg))
        .map(|dir| {
            println!(
                "cargo::rerun-if-changed={}",
                dir.join("Cargo.toml").display()
            );
            dir.join("src")
        })
}

/// The files of the crate a `use` names (`a_b`, the package `a_b` or `a-b`),
/// when Cargo.lock has it once: what [`inline_reexported_enums`] loads a
/// re-exported enum from. None for a crate not in the lockfile, or in it
/// twice.
pub fn dependency_files(
    krate: &str,
    locked: &BTreeMap<String, Vec<String>>,
) -> Option<Vec<syn::File>> {
    let pkg = [krate.to_string(), krate.replace('_', "-")]
        .into_iter()
        .find(|p| locked.get(p).is_some_and(|v| v.len() == 1))?;
    let dir = locate(&pkg, "lib.rs", locked);
    Some(parse_crate(&dir).into_iter().map(|(_, f)| f).collect())
}

/// Resolution order, most explicit first:
///   1. `<PKG>_SRC_DIR` env var (vendored checkout / git submodule anywhere)
///   2. a member of the workspace (a path dependency)
///   3. cargo registry cache, exact locked version
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
    if let Some(p) = workspace_member(pkg).filter(|p| p.join(marker).exists()) {
        return p;
    }

    // Only the registry cache is searched past this point: a vendored or git
    // dependency is never found there, and the way out is the override, so
    // every failure names it.
    let locked_ver = match locked.get(pkg).map(Vec::as_slice) {
        Some([v]) => v,
        Some(vs) => {
            panic!("`{pkg}` is in Cargo.lock twice ({vs:?}): set {key} to the checkout to read")
        }
        None => panic!("`{pkg}` is not in Cargo.lock: set {key} to its sources"),
    };
    registry_src(pkg, locked_ver)
        .filter(|p| p.join(marker).exists())
        .unwrap_or_else(|| {
            panic!(
                "cannot find the sources of `{pkg}` {locked_ver} (looked for {marker}).\n\
                 Run `cargo fetch` first. A vendored or git dependency is not in the \
                 registry cache: set {key} to its checkout (the directory holding {marker})."
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

/// Whether the public struct `ty` derives `tr` (`Clone`, say). Only the
/// derive is read: a hand-written impl is not looked for.
pub fn derives(files: &[syn::File], ty: &str, tr: &str) -> bool {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(|i| match i {
            Item::Struct(s) if s.ident == ty && is_pub(&s.vis) => Some(&s.attrs),
            _ => None,
        })
        .flatten()
        .filter(|a| a.path().is_ident("derive"))
        .any(|a| {
            let mut hit = false;
            // A derive list is paths: `Clone`, `std::clone::Clone`.
            let _ = a.parse_nested_meta(|m| {
                hit |= m.path.segments.last().is_some_and(|s| s.ident == tr);
                Ok(())
            });
            hit
        })
}

/// Whether the public struct `ty` has `Default`: derived, or an
/// `impl Default for ty`.
pub fn has_default(files: &[syn::File], ty: &str) -> bool {
    derives(files, ty, "Default")
        || files.iter().flat_map(|f| &f.items).any(|i| {
            matches!(i, Item::Impl(imp)
                if imp.trait_.as_ref().is_some_and(|(p, _)| {
                    p.segments.last().is_some_and(|s| s.ident == "Default")
                }) && self_ident(imp).is_some_and(|id| id == ty))
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

/// The public enum `files` define as `name`.
pub fn find_enum<'f>(files: &'f [syn::File], name: &str) -> Option<&'f syn::ItemEnum> {
    files.iter().flat_map(|f| &f.items).find_map(|i| match i {
        Item::Enum(e) if e.ident == name && is_pub(&e.vis) => Some(e),
        _ => None,
    })
}

/// The type an impl block is for, by its last path segment: upstream writes
/// `impl crate::Scene` as well as `impl Scene`.
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

/// Enums another crate defines and this one re-exports by name (`pub use
/// dep::Shade;`), copied into the file that re-exports them, under the name
/// the export gives: to everything after, they are this crate's, reachable
/// at the re-export's path, which is the path the generated code spells.
/// Enums only: a re-exported struct is that crate's class or data, already
/// read where it is defined. `load` hands back a dependency's files by the
/// crate name the `use` starts with, or None for one not available. Returns
/// the names copied.
pub fn inline_reexported_enums(
    parsed: &mut [(PathBuf, syn::File)],
    load: &mut dyn FnMut(&str) -> Option<Vec<syn::File>>,
) -> Vec<String> {
    let local: BTreeSet<String> = parsed
        .iter()
        .flat_map(|(_, f)| &f.items)
        .filter_map(|i| match i {
            Item::Mod(m) => Some(m.ident.to_string()),
            _ => None,
        })
        .collect();
    let mut loaded: BTreeMap<String, Option<Vec<syn::File>>> = BTreeMap::new();
    let mut out = Vec::new();
    for (_, file) in parsed.iter_mut() {
        let mut added = Vec::new();
        for item in &file.items {
            let Item::Use(u) = item else { continue };
            if !is_pub(&u.vis) {
                continue;
            }
            // (crate, name, as) for each `crate::..::Name [as K]` leaf.
            fn leaves(
                t: &syn::UseTree,
                path: &mut Vec<String>,
                out: &mut Vec<(String, String, String)>,
            ) {
                match t {
                    syn::UseTree::Path(p) => {
                        path.push(p.ident.to_string());
                        leaves(&p.tree, path, out);
                        path.pop();
                    }
                    syn::UseTree::Name(n) if !path.is_empty() && n.ident != "self" => {
                        out.push((path[0].clone(), n.ident.to_string(), n.ident.to_string()));
                    }
                    syn::UseTree::Rename(r) if !path.is_empty() && r.ident != "self" => {
                        out.push((path[0].clone(), r.ident.to_string(), r.rename.to_string()));
                    }
                    syn::UseTree::Group(g) => g.items.iter().for_each(|i| leaves(i, path, out)),
                    _ => {}
                }
            }
            let mut found = Vec::new();
            leaves(&u.tree, &mut Vec::new(), &mut found);
            for (krate, name, alias) in found {
                if ["crate", "self", "super"].contains(&krate.as_str()) || local.contains(&krate) {
                    continue;
                }
                let files = loaded.entry(krate.clone()).or_insert_with(|| load(&krate));
                let Some(e) = files.as_deref().and_then(|f| find_enum(f, &name)) else {
                    continue;
                };
                let mut e = e.clone();
                e.ident = Ident::new(&alias, e.ident.span());
                added.push(Item::Enum(e));
                out.push(alias);
            }
        }
        file.items.extend(added);
    }
    out
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
/// (`pub use other_crate;`) are left out, as before.
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
    /// `pub use other_crate::Width as Stretch`.
    pub renamed: BTreeMap<(PathBuf, String), String>,
    /// Every `pub const` an export reaches, as a caller of the crate writes
    /// it: (the namespace it is reachable at, `""` the root; its public name;
    /// the item, under that name). Root wins, as for types.
    pub consts: Vec<(String, String, syn::ItemConst)>,
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
            Const(Vec<String>, String),
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
                    Item::Const(c) if is_pub(&c.vis) => {
                        out.push((
                            c.ident.to_string(),
                            Target::Const(m.to_vec(), c.ident.to_string()),
                        ));
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
        // (module, const) -> (namespace, public name).
        let mut const_reached: BTreeMap<(Vec<String>, String), (String, String)> = BTreeMap::new();
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
                        // `root::Image` is what `Image` means.
                        let better = |old: &String| old.len() > ns.len();
                        if paths.get(&name).is_none_or(better) {
                            paths.insert(name.clone(), ns.clone());
                        }
                        let key = (def, ty);
                        if reached.get(&key).is_none_or(better) {
                            reached.insert(key, ns.clone());
                        }
                    }
                    Target::Const(def, c) => {
                        let key = (def, c);
                        if const_reached
                            .get(&key)
                            .is_none_or(|(old, _)| old.len() > ns.len())
                        {
                            const_reached.insert(key, (ns.clone(), name));
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
        let mut consts = Vec::new();
        for (module, file) in &modules {
            let Some(f) = files.get(file.as_path()) else {
                continue;
            };
            for item in &f.items {
                if let Item::Const(c) = item
                    && let Some((ns, name)) =
                        const_reached.get(&(module.clone(), c.ident.to_string()))
                {
                    let mut c = c.clone();
                    c.ident = Ident::new(name, c.ident.span());
                    consts.push((ns.clone(), name.clone(), c));
                }
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
            consts,
        }
    }

    /// The files of the crate as the generator should see them: every file
    /// the module tree reaches (a binary's `main.rs` is not one), with the
    /// `pub` types no export reaches taken out. Their `impl`s, functions and
    /// statics stay: `Scene::from_data` is implemented in a private `parser`,
    /// and a `const` table can live in a private `writer`.
    pub fn public_view(
        &self,
        parsed: &[(PathBuf, syn::File)],
        src: &Path,
    ) -> Vec<(PathBuf, syn::File)> {
        // An `impl` of a type no export reaches is no API either, and its
        // signatures would otherwise make what they mention look wanted:
        // a dependency's `Name::language()` pulled the crate's `Language` in.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn file(src: &str) -> syn::File {
        syn::parse_str(src).expect("a fixture that parses")
    }

    #[test]
    fn a_derive_is_read_by_its_last_segment() {
        let files = [file(
            "#[derive(Clone, std::marker::Copy)] pub struct Dot;
             #[derive(Clone)] struct Hidden;
             pub struct Frame;
             impl Default for Frame { fn default() -> Self { Frame } }
             #[derive(Default)] pub struct Options;",
        )];
        for (ty, tr, want) in [
            ("Dot", "Clone", true),
            ("Dot", "Copy", true),
            ("Dot", "Default", false),
            // Not public: not looked at.
            ("Hidden", "Clone", false),
            // A hand-written impl is no derive.
            ("Frame", "Default", false),
            ("Options", "Default", true),
            ("Grid", "Clone", false),
        ] {
            assert_eq!(derives(&files, ty, tr), want, "{ty}: {tr}");
        }
        for (ty, want) in [
            ("Options", true),
            ("Frame", true),
            ("Dot", false),
            ("Grid", false),
        ] {
            assert_eq!(has_default(&files, ty), want, "{ty}");
        }
    }

    #[test]
    fn an_impl_is_named_by_its_last_segment() {
        let f = file(
            "impl crate::Scene {} impl Scene {} impl Drop for Layer { fn drop(&mut self) {} }",
        );
        let names: Vec<String> = f
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Impl(imp) => self_ident(imp).map(ToString::to_string),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["Scene", "Scene", "Layer"]);
        let files = [f];
        assert_eq!(inherent_impls(&files, "Scene").count(), 2);
        assert_eq!(inherent_impls(&files, "Layer").count(), 0);
    }

    #[test]
    fn a_reexported_enum_is_read_as_its_own() {
        let mut parsed = vec![(
            PathBuf::from("/toy/src/lib.rs"),
            file(
                "mod alt;
                 pub use toy_dep::{Shade, Hue as Tint};
                 pub use toy_dep::Grid;
                 pub use alt::Color;
                 pub use crate::alt::Mark;
                 use toy_dep::Cell;",
            ),
        )];
        let mut asked = Vec::new();
        let mut load = |krate: &str| {
            asked.push(krate.to_string());
            (krate == "toy_dep").then(|| {
                vec![file(
                    "pub enum Shade { Light, Dark } pub enum Hue { Warm } pub struct Grid;
                     pub enum Cell { Empty }",
                )]
            })
        };
        let copied = inline_reexported_enums(&mut parsed, &mut load);
        // Renamed as re-exported; a struct, a private `use` and a local
        // module are not copied, and a crate is loaded once.
        assert_eq!(copied, ["Shade", "Tint"]);
        assert_eq!(asked, ["toy_dep"]);
        let enums: Vec<String> = parsed[0]
            .1
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Enum(e) => Some(e.ident.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(enums, ["Shade", "Tint"]);
    }

    /// The module tree decides what is public and where: by the exports,
    /// not by the files. In memory: a file module is looked for as
    /// `x/mod.rs` when no `x.rs` exists on disk.
    #[test]
    fn the_module_tree_follows_the_exports() {
        let src = Path::new("/toy/src");
        let parsed = vec![
            (
                src.join("lib.rs"),
                file(
                    "pub mod alt; mod inner;
                     pub use inner::Scene;
                     pub const NAME: &str = \"toy\";
                     pub struct Point;",
                ),
            ),
            (
                src.join("alt/mod.rs"),
                file("pub struct Color; pub const HIDDEN: u32 = 7;"),
            ),
            (
                src.join("inner/mod.rs"),
                file(
                    "pub struct Scene; pub struct Hidden;
                     impl Hidden { pub fn peek(&self) {} }
                     impl Scene { pub fn width(&self) -> u32 { 0 } }",
                ),
            ),
        ];
        let tree = ModTree::load(src, &parsed);
        let paths: Vec<(&str, &str)> = tree
            .paths
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(paths, [("Color", "alt"), ("Point", ""), ("Scene", "")]);
        assert!(
            tree.private
                .contains(&(src.join("inner/mod.rs"), "Hidden".to_string()))
        );
        let consts: Vec<(&str, &str)> = tree
            .consts
            .iter()
            .map(|(m, n, _)| (m.as_str(), n.as_str()))
            .collect();
        assert!(consts.contains(&("", "NAME")), "{consts:?}");
        assert!(consts.contains(&("alt", "HIDDEN")), "{consts:?}");
        // The private type goes, and its impl with it; the exported one
        // and its impl stay.
        let view = tree.public_view(&parsed, src);
        let inner = &view
            .iter()
            .find(|(p, _)| *p == src.join("inner/mod.rs"))
            .unwrap()
            .1;
        let kept: Vec<String> = inner.items.iter().filter_map(declared).collect();
        assert_eq!(kept, ["Scene", "Scene"]);
    }
}
