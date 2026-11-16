//! Generates the whole NAPI binding layer from the *actual* resvg / usvg /
//! fontdb sources: `syn` parses them, `quote` re-emits Rust.
//!
//! The generator is its own crate, `crates/napi-gen`, and runs every phase
//! in order (`napi_gen::generate::generate`); this one only says what to
//! run it on:
//!
//!   build/config.rs    the resvg-specific lists: the crates, the allowlists,
//!                      the hooks, and the `napi_gen::Config` they make. What
//!                      another crate would rewrite
//!   build/template.rs  the resvg-specific items of the generated file,
//!                      written by hand: the one converter
//!   src/guard.rs       the hand-written providers (`config::GEN.hand_written`)
//!
//! The split is verifiable rather than a matter of taste: src/lib.rs,
//! index.d.ts and codegen-report.txt are committed and CI diffs them, so a
//! refactor here is correct exactly when the output does not move.

// The resvg-specific files. `#[path]` because Cargo compiles this one as a
// crate root, so a plain `mod` would look for the file beside it -- in the
// repository root.
#[path = "build/config.rs"]
mod config;
#[path = "build/template.rs"]
mod template;

fn main() {
    napi_build::setup();
    println!("cargo::rerun-if-changed=build.rs");
    // Cargo watches only what it is told: without these, editing config or the
    // template would leave src/lib.rs stale and the CI drift check would fail
    // on someone else's machine rather than here. napi-gen, a path dependency,
    // Cargo tracks on its own.
    for m in ["config", "template"] {
        println!("cargo::rerun-if-changed=build/{m}.rs");
    }
    napi_gen::generate::generate(&config::GEN, template::template(), quote::quote!());
}
