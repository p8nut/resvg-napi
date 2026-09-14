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
    let Ok(text) = fs::read_to_string(&lock) else {
        return out;
    };
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("version = ") {
            if let Some(n) = name.take() {
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
pub fn registry_src(pkg: &str, want: Option<&str>) -> Option<PathBuf> {
    let mut best: Option<((u64, u64, u64), PathBuf)> = None;
    for index in fs::read_dir(cargo_home().join("registry/src"))
        .ok()?
        .flatten()
    {
        let Ok(entries) = fs::read_dir(index.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let dir = e.file_name().to_string_lossy().to_string();
            let Some(ver) = dir.strip_prefix(&format!("{pkg}-")) else {
                continue;
            };
            // "usvg-0.48.1" is ours, "usvg-parser-0.44.0" is not.
            if !ver.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let src = e.path().join("src");
            if want == Some(ver) {
                return Some(src);
            }
            let key = semver_key(ver);
            if best.as_ref().is_none_or(|(b, _)| key > *b) {
                best = Some((key, src));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Resolution order, most explicit first:
///   1. `<PKG>_SRC_DIR` env var (vendored checkout / git submodule anywhere)
///   2. `vendor/resvg/crates/<pkg>/src` (the upstream monorepo as a submodule)
///   3. cargo registry cache, exact locked version
///   4. cargo registry cache, newest version present
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
    if vendored.join(marker).exists() {
        return vendored;
    }

    let locked_ver = locked.get(pkg).map(String::as_str);
    registry_src(pkg, locked_ver)
        .or_else(|| registry_src(pkg, None))
        .filter(|p| p.join(marker).exists())
        .unwrap_or_else(|| {
            panic!(
                "cannot find the sources of `{pkg}` (looked for {marker}).\n\
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
        if let Item::Use(u) = item {
            if matches!(u.vis, Visibility::Public(_)) {
                walk(&u.tree, &mut Vec::new(), &mut out);
            }
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
            if let syn::Type::Path(p) = &mut *im.self_ty {
                if let Some(seg) = p.path.segments.last_mut() {
                    to_ident(&mut seg.ident);
                }
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

/// Only the enums actually referenced by `Options` get mirrored — the SVG tree
/// enums (Paint, Node, ...) carry payloads and stay on the Rust side.
/// Public module names declared anywhere in the crate (`pub mod filter;`).
/// A type defined in `filter.rs` is then reachable as `usvg::filter::X`.
pub fn public_modules(files: &[syn::File]) -> BTreeSet<String> {
    files
        .iter()
        .flat_map(|f| &f.items)
        .filter_map(|i| match i {
            Item::Mod(m) if is_pub(&m.vis) => Some(m.ident.to_string()),
            _ => None,
        })
        .collect()
}
