//! Code emitted by `ironsbe-codegen` in panic-free mode
//! (`GeneratorOptions::with_panicking_setters(false)`), compiled with no
//! `#[allow]` under the strict lint bar of this crate's `Cargo.toml`
//! (`missing_docs`, `unsafe_code`, `clippy::indexing_slicing`,
//! `clippy::panic`, `clippy::unwrap_used`, `clippy::expect_used` and the
//! rest at deny, and every default lint as an error under
//! `cargo clippy -D warnings`). A downstream crate that compiles these
//! codecs into its own library holds them to that bar (karyos#646). Not
//! published.

/// Codecs of Real Logic's example schema, `Car` version 0.
pub mod car_v0 {
    include!(concat!(env!("OUT_DIR"), "/car_v0.rs"));
}

/// Codecs of Real Logic's example extension schema, `Car` version 1.
pub mod car_v1 {
    include!(concat!(env!("OUT_DIR"), "/car_v1.rs"));
}
