//! What the generated file holds beside the generated items, written by hand:
//! the paths to the wrapped crates, the `guard` module, and the one converter
//! `config::GEN` names. Everything generic (the napi imports, `to_f32`, the
//! log functions) is `napi_gen::emit::prelude`.

use proc_macro2::TokenStream;
use quote::quote;

/// The resvg-specific items of `src/lib.rs`.
pub fn template() -> TokenStream {
    quote! {
        use resvg::{tiny_skia, usvg};

        #[doc = " The resvg-specific providers (`guard.*`), written by hand: src/guard.rs."]
        mod guard;
        use guard::{GuardFontResolver, GuardImageHrefResolver};

        #[doc = " A number of spaces (0-255), `'tabs'` or `'none'` as a `usvg::Indent`:"]
        #[doc = " `config::GEN.converters`. `Indent` is xmlwriter's, a crate the"]
        #[doc = " generator does not read."]
        fn indent_from_js(v: &Either<u32, String>, name: &str) -> Result<usvg::Indent> {
            match v {
                Either::A(n) => u8::try_from(*n)
                    .map(usvg::Indent::Spaces)
                    .map_err(|_| Error::from_reason(format!("invalid {name}: {n}"))),
                Either::B(s) => match s.as_str() {
                    "tabs" => Ok(usvg::Indent::Tabs),
                    "none" => Ok(usvg::Indent::None),
                    _ => Err(Error::from_reason(format!("invalid {name}: {s:?}"))),
                },
            }
        }
    }
}
