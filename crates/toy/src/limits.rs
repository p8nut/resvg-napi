//! A private module: what the crate root re-exports of it is public, the
//! rest is not, as for a type.

/// How many layers a scene may hold: `toy::MAX_LAYERS`.
pub const MAX_LAYERS: u32 = 64;

/// `pub`, but exported by nothing: no `toy.HIDDEN`.
#[allow(dead_code)]
pub const HIDDEN: u32 = 7;
