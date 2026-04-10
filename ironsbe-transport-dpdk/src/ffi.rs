//! Minimal hand-written FFI declarations for the DPDK functions used
//! by this crate.
//!
//! We declare only the functions we call rather than running `bindgen`
//! over the full DPDK header tree (~50k lines).  This keeps the build
//! simple, reproducible, and avoids pulling in a C compiler at build
//! time for bindgen.
//!
//! All types and constants are compatible with DPDK 23.11 LTS.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

// =====================================================================
// Opaque types — we only ever pass pointers, never inspect fields.
// =====================================================================

/// Opaque mempool.
#[repr(C)]
pub struct rte_mempool {
    _opaque: [u8; 0],
}

/// Opaque mbuf (message buffer / packet).
#[repr(C)]
pub struct rte_mbuf {
    _opaque: [u8; 0],
}

/// Ethernet device configuration.
#[repr(C)]
#[derive(Default)]
pub struct rte_eth_conf {
    /// We zero-init the entire struct (440 bytes in DPDK 23.11) and
    /// let DPDK fill defaults.  The struct is large and mostly
    /// zero-initialised in practice.
    pub _pad: [u8; 512],
}

/// Ethernet device info (returned by `rte_eth_dev_info_get`).
#[repr(C)]
pub struct rte_eth_dev_info {
    pub _pad: [u8; 512],
}

// =====================================================================
// EAL
// =====================================================================

extern "C" {
    /// Initialise the DPDK EAL (Environment Abstraction Layer).
    ///
    /// Must be called exactly once, before any other DPDK function.
    /// Returns the number of parsed args on success, or a negative
    /// errno on failure.
    pub fn rte_eal_init(argc: c_int, argv: *mut *mut c_char) -> c_int;

    /// Cleanly shut down the EAL.
    pub fn rte_eal_cleanup() -> c_int;

    /// Returns the NUMA socket id of the calling lcore.
    pub fn rte_socket_id() -> c_uint;
}

// =====================================================================
// Mempool
// =====================================================================

extern "C" {
    /// Creates a mempool for packet buffers (mbufs).
    pub fn rte_pktmbuf_pool_create(
        name: *const c_char,
        n: c_uint,
        cache_size: c_uint,
        priv_size: u16,
        data_room_size: u16,
        socket_id: c_int,
    ) -> *mut rte_mempool;

    /// Frees a mempool.
    pub fn rte_mempool_free(mp: *mut rte_mempool);
}

// =====================================================================
// Ethdev — port configuration and rx/tx
// =====================================================================

extern "C" {
    /// Returns the number of available ethernet ports.
    pub fn rte_eth_dev_count_avail() -> u16;

    /// Configures an ethernet device.
    pub fn rte_eth_dev_configure(
        port_id: u16,
        nb_rx_queue: u16,
        nb_tx_queue: u16,
        eth_conf: *const rte_eth_conf,
    ) -> c_int;

    /// Sets up an RX queue.
    pub fn rte_eth_rx_queue_setup(
        port_id: u16,
        rx_queue_id: u16,
        nb_rx_desc: u16,
        socket_id: c_uint,
        rx_conf: *const c_void, // NULL for defaults
        mb_pool: *mut rte_mempool,
    ) -> c_int;

    /// Sets up a TX queue.
    pub fn rte_eth_tx_queue_setup(
        port_id: u16,
        tx_queue_id: u16,
        nb_tx_desc: u16,
        socket_id: c_uint,
        tx_conf: *const c_void, // NULL for defaults
    ) -> c_int;

    /// Starts the ethernet device.
    pub fn rte_eth_dev_start(port_id: u16) -> c_int;

    /// Stops the ethernet device.
    pub fn rte_eth_dev_stop(port_id: u16) -> c_int;

    /// Retrieves a burst of input packets from an RX queue.
    pub fn rte_eth_rx_burst(
        port_id: u16,
        queue_id: u16,
        rx_pkts: *mut *mut rte_mbuf,
        nb_pkts: u16,
    ) -> u16;

    /// Sends a burst of output packets on a TX queue.
    pub fn rte_eth_tx_burst(
        port_id: u16,
        queue_id: u16,
        tx_pkts: *mut *mut rte_mbuf,
        nb_pkts: u16,
    ) -> u16;

    /// Promiscuous mode enable.
    pub fn rte_eth_promiscuous_enable(port_id: u16) -> c_int;
}

// =====================================================================
// Mbuf access
// =====================================================================

extern "C" {
    /// Allocates an mbuf from a mempool.
    pub fn rte_pktmbuf_alloc(pool: *mut rte_mempool) -> *mut rte_mbuf;

    /// Frees an mbuf back to its pool.
    pub fn rte_pktmbuf_free(m: *mut rte_mbuf);

    /// Returns a pointer to the start of the data in the mbuf.
    ///
    /// Note: in DPDK this is actually an inline function / macro
    /// (`rte_pktmbuf_mtod`).  We use the underlying field access
    /// through [`mbuf_data_ptr`] below instead.
    pub fn rte_pktmbuf_append(m: *mut rte_mbuf, len: u16) -> *mut c_char;

    /// Returns the data length of the mbuf.
    pub fn rte_pktmbuf_data_len(m: *const rte_mbuf) -> u16;
}

/// Reads the raw data pointer from an mbuf by computing the
/// `buf_addr + data_off` offset.
///
/// # Safety
/// `m` must be a valid, non-null mbuf pointer.  The returned slice
/// is valid for `rte_pktmbuf_data_len(m)` bytes.
#[inline]
pub unsafe fn mbuf_data_ptr(m: *const rte_mbuf) -> *const u8 {
    // rte_mbuf layout (DPDK 23.11):
    //   offset 0:   buf_addr  (*mut c_void)
    //   offset 8:   buf_iova  (u64)
    //   offset 16:  rearm_data ...
    //   offset 16+2: data_off (u16)
    //
    // rte_pktmbuf_mtod(m) = (char*)m->buf_addr + m->data_off
    let buf_addr = *(m as *const *const u8);
    let data_off = *((m as *const u8).add(18) as *const u16);
    buf_addr.add(data_off as usize)
}
