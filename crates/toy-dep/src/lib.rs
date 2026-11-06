//! What `toy` re-exports by name (`pub use toy_dep::Shade`): an enum another
//! crate defines, which a binding of `toy` reads as `toy::Shade`.

/// A colour by kind or by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shade {
    /// No colour.
    Plain,
    /// A named colour.
    Named(String),
}
