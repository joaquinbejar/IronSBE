//! Compile-level tests for code emitted by `ironsbe-codegen`.
//!
//! `build.rs` runs the generator on an inline schema and writes the output to
//! `OUT_DIR`; this crate includes it so the integration tests in `tests/`
//! exercise real generated codecs end to end. Not published.

/// Codecs generated at build time from the var data schema in `build.rs`.
///
/// Generated code is exempt from this crate's lint bar; tightening the
/// generator output is tracked separately.
#[allow(dead_code, unused_imports, missing_docs, clippy::all)]
pub mod var_data {
    include!(concat!(env!("OUT_DIR"), "/var_data_schema.rs"));
}
