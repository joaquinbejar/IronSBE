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
    // Bindgen-generated unsafe functions don't have # Safety docs.
    clippy::missing_safety_doc,
    clippy::missing_docs_in_private_items,
    clippy::ptr_offset_with_cast,
)]

// Include the bindgen output.
include!(concat!(env!("OUT_DIR"), "/dpdk_bindings.rs"));

// C shim functions wrapping inline / macro DPDK accessors.
//
// These are compiled from `shim.c` by the `cc` crate and linked into
// this crate.  They exist because bindgen cannot generate Rust
// bindings for `static inline` functions or preprocessor macros.
unsafe extern "C" {
    // --- mbuf accessors ---
    pub fn ironsbe_pktmbuf_mtod(m: *const rte_mbuf) -> *const u8;
    pub fn ironsbe_pktmbuf_data_len_shim(m: *const rte_mbuf) -> u16;
    pub fn ironsbe_pktmbuf_alloc(pool: *mut rte_mempool) -> *mut rte_mbuf;
    pub fn ironsbe_pktmbuf_free(m: *mut rte_mbuf);
    pub fn ironsbe_pktmbuf_append(m: *mut rte_mbuf, len: u16) -> *mut core::ffi::c_char;
    // --- ethdev rx/tx burst ---
    pub fn ironsbe_eth_rx_burst(
        port_id: u16,
        queue_id: u16,
        rx_pkts: *mut *mut rte_mbuf,
        nb_pkts: u16,
    ) -> u16;
    pub fn ironsbe_eth_tx_burst(
        port_id: u16,
        queue_id: u16,
        tx_pkts: *mut *mut rte_mbuf,
        nb_pkts: u16,
    ) -> u16;
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
