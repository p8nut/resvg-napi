//! What the generated file holds beside the generated items, written by hand:
//! the one converter `config::GEN` names. Everything generic (the napi
//! imports, `to_f32`, the log functions) is `napi_gen::emit::prelude`; the
//! crate paths and the `guard` module are `config::GEN.uses` and
//! `config::GEN.hand_written`.

use proc_macro2::TokenStream;
use quote::quote;

/// The resvg-specific items of `src/lib.rs`.
pub fn template() -> TokenStream {
    quote! {
        #[doc = " A number of spaces (0-255), `'tabs'` or `'none'` as a `usvg::Indent`:"]
        #[doc = " `config::GEN.converters`. `Indent` is xmlwriter's, a crate the"]
        #[doc = " generator does not read."]
        fn indent_from_js(
            v: &Either<f64, String>,
            name: &str,
        ) -> std::result::Result<usvg::Indent, napi_support::Thrown> {
            match v {
                Either::A(n) => napi_support::int(name, *n).map(usvg::Indent::Spaces),
                Either::B(s) => match s.as_str() {
                    "tabs" => Ok(usvg::Indent::Tabs),
                    "none" => Ok(usvg::Indent::None),
                    _ => Err(napi_support::invalid(name, format!("{s:?}"))),
                },
            }
        }
    }
}
