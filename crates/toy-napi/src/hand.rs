//! The toy binding's providers, written by hand (`GEN.hand_written`): the
//! generator does not touch this file, it declares it and imports the class
//! `GEN.providers` names.
//!
//! - `hand.Shout`: supplies `toy.ReportOptions.finish`, a function pointer,
//!   which has no JS form and no class to derive a provider from.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use napi_support::Slot;

/// Writes a report's finished line in capitals: the provider for
/// `toy.ReportOptions.finish`.
#[napi(namespace = "hand", js_name = "Shout")]
pub struct HandShout {
    inner: Slot<fn(String) -> String>,
}

#[napi(namespace = "hand")]
impl HandShout {
    #[allow(clippy::new_without_default)] // a JS constructor
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Slot::new(|s| s.to_uppercase()),
        }
    }

    /// Every later call throws `Disposed`, and so does passing it to a
    /// report. Harmless twice.
    #[napi]
    pub fn dispose(&mut self) -> std::result::Result<(), Error<String>> {
        self.inner.dispose("Shout")
    }

    /// Whether `dispose()` has been called.
    #[napi(getter)]
    pub fn disposed(&self) -> bool {
        self.inner.disposed()
    }
}

impl HandShout {
    /// The function a report's `finish` takes.
    pub(crate) fn finish(&self) -> std::result::Result<fn(String) -> String, Error<String>> {
        Ok(*self.inner.get("Shout")?)
    }
}
