//! Bindgen-generated DPDK FFI bindings + C shim functions.
//!
//! The bindings are generated at build time from the DPDK headers
//! installed via `libdpdk-dev` (DPDK ≥ 23.11).  Only the functions
//! and types actually used by this crate are included (see
//! `build.rs` allowlists).
//!
//! The C shim (`shim.c`) exposes `rte_pktmbuf_mtod` and
//! `rte_pktmbuf_data_len` — which are macros / inline functions in
//! the DPDK headers — as real `extern "C"` functions callable from
//! Rust.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    // Bindgen generates `unsafe fn` bodies that call other unsafe
    // functions without explicit `unsafe {}` blocks (Rust 2024 default).
    unsafe_op_in_unsafe_fn,
    // bindgen generates types that may trigger these:
    clippy::useless_transmute,
    clippy::unnecessary_cast,
    clippy::too_many_arguments,
    clippy::redundant_static_lifetimes,
    // Some generated Default impls reference large arrays.
    clippy::derivable_impls,
)]

// Include the bindgen output.
include!(concat!(env!("OUT_DIR"), "/dpdk_bindings.rs"));

// C shim functions wrapping macro-based DPDK accessors.
unsafe extern "C" {
    /// Returns a `const void*` to the start of the data in the mbuf.
    ///
    /// Wraps the `rte_pktmbuf_mtod` macro via `shim.c`.
    pub fn ironsbe_pktmbuf_mtod(m: *const rte_mbuf) -> *const u8;

    /// Returns the data length of the mbuf.
    ///
    /// Wraps the `rte_pktmbuf_data_len` inline function via `shim.c`.
    pub fn ironsbe_pktmbuf_data_len_shim(m: *const rte_mbuf) -> u16;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rte_eth_conf_is_not_zero_sized() {
        assert!(
            std::mem::size_of::<rte_eth_conf>() > 0,
            "rte_eth_conf should have a non-zero size from the real DPDK header"
        );
    }

    #[test]
    fn test_rte_mbuf_has_expected_minimum_size() {
        // rte_mbuf is at least 128 bytes in all DPDK versions.
        assert!(
            std::mem::size_of::<rte_mbuf>() >= 64,
            "rte_mbuf size {} is suspiciously small",
            std::mem::size_of::<rte_mbuf>()
        );
    }

    #[test]
    fn test_rte_mempool_is_not_zero_sized() {
        assert!(
            std::mem::size_of::<rte_mempool>() > 0,
            "rte_mempool should have a non-zero size"
        );
    }
}
