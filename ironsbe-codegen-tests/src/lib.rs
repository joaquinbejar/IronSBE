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

/// Codecs generated from the same schema with
/// `GeneratorOptions::with_panicking_setters(false)` (#70, #74).
///
/// The panic-prone lints are denied here so `cargo clippy` fails if the
/// generator ever emits a `panic!`, an `unwrap`/`expect` or an unchecked
/// slice index in this mode. `tests/panic_free.rs` drives these codecs over
/// truncated and corrupted frames to check the runtime side.
#[allow(dead_code, unused_imports, missing_docs, clippy::all)]
#[deny(
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used
)]
pub mod var_data_no_panic {
    include!(concat!(env!("OUT_DIR"), "/var_data_schema_no_panic.rs"));
}

/// Codecs generated at build time from the versioned schema in `build.rs`.
#[allow(dead_code, unused_imports, missing_docs, clippy::all)]
pub mod versioned {
    include!(concat!(env!("OUT_DIR"), "/versioned_schema.rs"));
}
