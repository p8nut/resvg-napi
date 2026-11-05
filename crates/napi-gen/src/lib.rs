//! Generates a napi-rs binding layer from the *actual* sources of the crates
//! it wraps: `syn` parses them, `quote` re-emits Rust. Used from a build
//! script.
//!
//! One phase per module:
//!
//!   sources  finding and parsing upstream (env override -> cargo registry)
//!   vocab    what a Rust type means on the JS side -- newtypes, aliases,
//!            `Deref` chains, payload enums, naming
//!   emit     turning those answers into tokens napi can expand
//!   cfg      which upstream methods exist on which target
//!
//! What is specific to the wrapped crates is not here: the build script fills
//! a [`Config`] and hands it over with [`configure`] before any phase runs.

use std::sync::OnceLock;

/// The codegen's own report: what it derived from upstream, and what it left
/// alone with the reason. Cargo gives a build script no channel but
/// `cargo::warning`, so this competes with real warnings -- clippy's four were
/// buried under 101 lines of it. Off unless [`Config::log_env`] is set.
#[macro_export]
macro_rules! report {
    ($($arg:tt)*) => {
        if ::std::env::var_os($crate::config().log_env).is_some() {
            println!("cargo::warning={}", format!($($arg)*));
        }
    };
}

pub mod cfg;
pub mod emit;
pub mod sources;
pub mod vocab;

/// What the phases need to know about the wrapped crates and do not discover
/// on their own. Each list is an API decision, not a description of upstream:
/// the build script owns the values, and says why each one is there.
pub struct Config {
    /// The env var that turns [`report!`] on.
    pub log_env: &'static str,
    /// An option field indexing an upstream table without a bounds check, by
    /// its name suffix, and the table. Clamped to the table's length.
    pub clamp: (&'static str, &'static str),
    /// A struct with these fields is a transform, mapped to `Matrix`.
    pub matrix_fields: &'static [&'static str],
    /// Upstream type spellings for the template's domain shapes.
    pub size_types: &'static [&'static str],
    pub rect_types: &'static [&'static str],
    pub opt_rect_types: &'static [&'static str],
    pub path_data_types: &'static [&'static str],
    pub tag4_types: &'static [&'static str],
    /// An argument type JS hands over as a class instance instead: (the
    /// upstream type, the class).
    pub arg_standin: (&'static str, &'static str),
    /// Option fields the template wires by hand; every other unmappable field
    /// is dropped.
    pub hand_wired_fields: &'static [&'static str],
    /// The tree's own types: never followed as data.
    pub node_types: &'static [&'static str],
    /// Conversions, not data: members never counted as unmapped data.
    pub not_data: &'static [&'static str],
    /// Upstream handle types the template wraps by hand, with their class.
    pub hand_wrapped: &'static [(&'static str, &'static str)],
    /// Data types emitted under another name.
    pub data_renames: &'static [(&'static str, &'static str)],
    /// Enums emitted under another name.
    pub enum_renames: &'static [(&'static str, &'static str)],
    /// Upstream Cargo features an emitted method can be gated on, and what
    /// each is for the binding crate: `"true"` (always built) or a cfg it is
    /// built under. A gate listed nowhere fails the build.
    pub features: &'static [(&'static str, &'static str)],
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Hands the generator its config: once, before any phase runs.
pub fn configure(config: Config) {
    assert!(
        CONFIG.set(config).is_ok(),
        "napi_gen::configure called twice"
    );
}

/// The config [`configure`] was given.
pub fn config() -> &'static Config {
    CONFIG
        .get()
        .expect("napi_gen::configure must run before the generator does")
}

/// `name` through a rename table, or unchanged.
pub fn renamed<'a>(table: &[(&str, &'a str)], name: &'a str) -> &'a str {
    table
        .iter()
        .find(|(from, _)| *from == name)
        .map_or(name, |(_, to)| to)
}
