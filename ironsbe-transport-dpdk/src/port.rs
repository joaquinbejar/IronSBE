//! DPDK port (ethdev) configuration and rx/tx burst helpers.
//!
//! A "port" in DPDK terms is a network interface managed by a
//! poll-mode driver (PMD).  When using the `net_af_xdp` PMD the port
//! represents an AF_XDP socket on an existing kernel NIC — the NIC
//! stays in the kernel and is shared.

use crate::ffi;
use std::ffi::CString;
use std::io;
use std::ptr;

/// Maximum burst size for rx/tx operations.
pub const MAX_BURST: usize = 32;

/// Default number of descriptors per rx/tx queue.
const DEFAULT_NB_DESC: u16 = 1024;

/// Default mbuf pool size.
const DEFAULT_POOL_SIZE: u32 = 8191;

/// Default per-core mbuf cache size.
const DEFAULT_CACHE_SIZE: u32 = 250;

/// Default mbuf data room size (headroom + MTU).
const DEFAULT_DATA_ROOM: u16 = 2048 + 128; // RTE_PKTMBUF_HEADROOM + MTU

/// A configured DPDK port with one rx and one tx queue.
pub struct DpdkPort {
    port_id: u16,
    pool: *mut ffi::rte_mempool,
}

impl DpdkPort {
    /// Configures and starts the first available DPDK port.
    ///
    /// After EAL init with a `--vdev=net_af_xdp0,...` argument, this
    /// function finds port 0, creates a mempool, sets up one rx + one
    /// tx queue, enables promiscuous mode, and starts the port.
    ///
    /// # Errors
    /// Returns an `io::Error` if any DPDK call fails.
    pub fn init() -> io::Result<Self> {
        let nb_ports = unsafe { ffi::rte_eth_dev_count_avail() };
        if nb_ports == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no DPDK ports available (did you pass --vdev=net_af_xdp0,...?)",
            ));
        }

        let port_id: u16 = 0;
        let socket_id = unsafe { ffi::rte_socket_id() };

        // Create mbuf pool.
        let pool_name =
            CString::new("MBUF_POOL").map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let pool = unsafe {
            ffi::rte_pktmbuf_pool_create(
                pool_name.as_ptr(),
                DEFAULT_POOL_SIZE,
                DEFAULT_CACHE_SIZE,
                0,
                DEFAULT_DATA_ROOM,
                socket_id as i32,
            )
        };
        if pool.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "rte_pktmbuf_pool_create returned NULL",
            ));
        }

        // Configure the port with 1 rx + 1 tx queue.
        let eth_conf = ffi::rte_eth_conf::default();
        let ret = unsafe { ffi::rte_eth_dev_configure(port_id, 1, 1, &eth_conf) };
        if ret != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("rte_eth_dev_configure failed: {ret}"),
            ));
        }

        // Setup rx queue.
        let ret = unsafe {
            ffi::rte_eth_rx_queue_setup(
                port_id,
                0,
                DEFAULT_NB_DESC,
                socket_id,
                ptr::null(),
                pool,
            )
        };
        if ret != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("rte_eth_rx_queue_setup failed: {ret}"),
            ));
        }

        // Setup tx queue.
        let ret = unsafe {
            ffi::rte_eth_tx_queue_setup(port_id, 0, DEFAULT_NB_DESC, socket_id, ptr::null())
        };
        if ret != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("rte_eth_tx_queue_setup failed: {ret}"),
            ));
        }

        // Enable promiscuous mode (receive all packets, important for
        // AF_XDP PMD to see traffic for our MAC/IP).
        let ret = unsafe { ffi::rte_eth_promiscuous_enable(port_id) };
        if ret != 0 {
            tracing::warn!("rte_eth_promiscuous_enable failed: {ret} (non-fatal)");
        }

        // Start the port.
        let ret = unsafe { ffi::rte_eth_dev_start(port_id) };
        if ret != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("rte_eth_dev_start failed: {ret}"),
            ));
        }

        tracing::info!(port_id, "DPDK port started");
        Ok(Self { port_id, pool })
    }

    /// Returns the port id.
    #[must_use]
    pub fn port_id(&self) -> u16 {
        self.port_id
    }

    /// Returns the mbuf pool pointer (needed for allocating tx mbufs).
    #[must_use]
    pub fn pool(&self) -> *mut ffi::rte_mempool {
        self.pool
    }

    /// Receives a burst of packets from the rx queue.
    ///
    /// Returns the number of packets received (≤ `MAX_BURST`).  The
    /// caller must process and free the mbufs.
    ///
    /// # Safety
    /// `pkts` must point to an array of at least `MAX_BURST` mbuf
    /// pointers.
    #[inline]
    pub unsafe fn rx_burst(&self, pkts: &mut [*mut ffi::rte_mbuf; MAX_BURST]) -> u16 {
        // SAFETY: caller guarantees the array is large enough.
        unsafe {
            ffi::rte_eth_rx_burst(self.port_id, 0, pkts.as_mut_ptr(), MAX_BURST as u16)
        }
    }

    /// Sends a burst of packets on the tx queue.
    ///
    /// Returns the number of packets successfully enqueued.  The
    /// caller must free any mbufs that were NOT sent (i.e.
    /// `pkts[sent..nb_pkts]`).
    ///
    /// # Safety
    /// `pkts` must contain valid mbuf pointers.
    #[inline]
    pub unsafe fn tx_burst(&self, pkts: &mut [*mut ffi::rte_mbuf], nb_pkts: u16) -> u16 {
        // SAFETY: caller guarantees valid mbuf pointers.
        unsafe { ffi::rte_eth_tx_burst(self.port_id, 0, pkts.as_mut_ptr(), nb_pkts) }
    }
}

impl Drop for DpdkPort {
    fn drop(&mut self) {
        unsafe {
            let _ = ffi::rte_eth_dev_stop(self.port_id);
            if !self.pool.is_null() {
                ffi::rte_mempool_free(self.pool);
            }
        }
        tracing::info!(port_id = self.port_id, "DPDK port stopped");
    }
}
