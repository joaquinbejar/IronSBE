//! High-level [`LocalTransport`] implementation for the DPDK backend.
//!
//! `DpdkTransport<S>` ties together DPDK EAL + port + a user-selected
//! [`XdpStack`] into a single type usable by `LocalServer`.
//!
//! The DPDK datapath runs a busy-poll loop calling
//! `rte_eth_rx_burst` / `rte_eth_tx_burst` and feeding received
//! frames into the stack's `on_rx`.

use crate::eal::Eal;
use crate::ffi;
use crate::port::{DpdkPort, MAX_BURST};
use ironsbe_transport::traits::{LocalListener, LocalTransport};
use ironsbe_transport::xdp::stack::{FrameTxQueue, XdpStack};
use std::ffi::CString;
use std::io;
use std::marker::PhantomData;
use std::net::{IpAddr, SocketAddr};

/// Configuration for [`DpdkTransport`].
#[derive(Debug, Clone)]
pub struct DpdkConfig<S> {
    /// EAL arguments passed to `rte_eal_init`.
    ///
    /// Must include a `--vdev=net_af_xdp0,iface=<iface>,...` argument
    /// for the AF_XDP PMD mode.  Example:
    ///
    /// ```text
    /// ["ironsbe", "--no-huge", "--proc-type=primary",
    ///  "--vdev=net_af_xdp0,iface=eth0,start_queue=0,queue_count=1"]
    /// ```
    pub eal_args: Vec<String>,
    /// The userspace stack (same trait as the XDP backend).
    pub stack: S,
    /// Listen port (for `local_addr()` reporting).
    pub listen_port: u16,
}

impl<S: Clone> DpdkConfig<S> {
    /// Creates a new DPDK config.
    #[must_use]
    pub fn new(eal_args: Vec<String>, stack: S, listen_port: u16) -> Self {
        Self {
            eal_args,
            stack,
            listen_port,
        }
    }
}

/// Fallback `From<SocketAddr>` — builds a minimal AF_XDP PMD config
/// on `lo`.  Only useful for tests; production callers should construct
/// an explicit [`DpdkConfig`].
impl From<SocketAddr> for DpdkConfig<ironsbe_transport::xdp::UdpStack> {
    fn from(addr: SocketAddr) -> Self {
        use ironsbe_transport::xdp::stack::udp::UdpStackConfig;
        let ip = match addr.ip() {
            IpAddr::V4(v4) => v4,
            IpAddr::V6(_) => std::net::Ipv4Addr::LOCALHOST,
        };
        let mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let stack = ironsbe_transport::xdp::UdpStack::new(UdpStackConfig::new(
            ip,
            addr.port(),
            mac,
        ));
        Self {
            eal_args: vec![
                "ironsbe".into(),
                "--no-huge".into(),
                "--proc-type=primary".into(),
                "--vdev=net_af_xdp0,iface=lo,start_queue=0,queue_count=1".into(),
            ],
            stack,
            listen_port: addr.port(),
        }
    }
}

/// DPDK transport backend.
///
/// Generic over the userspace stack `S` (reuses the same [`XdpStack`]
/// trait as the AF_XDP backend, since both operate on raw Ethernet
/// frames).
pub struct DpdkTransport<S: XdpStack>(PhantomData<S>);

impl<S> LocalTransport for DpdkTransport<S>
where
    S: XdpStack + Clone + 'static,
    S::Connection: 'static,
    S::Error: std::fmt::Display + 'static,
    DpdkConfig<S>: From<SocketAddr> + Clone + 'static,
{
    type Listener = DpdkListener<S>;
    type Connection = S::Connection;
    type Error = io::Error;
    type BindConfig = DpdkConfig<S>;
    type ConnectConfig = DpdkConfig<S>;

    async fn bind_with(config: DpdkConfig<S>) -> io::Result<DpdkListener<S>> {
        let arg_refs: Vec<&str> = config.eal_args.iter().map(|s| s.as_str()).collect();
        let eal = Eal::init(&arg_refs)?;
        let port = DpdkPort::init()?;
        let local_ip = config.stack.local_ip();
        let listen_port = config.listen_port;
        Ok(DpdkListener {
            _eal: eal,
            port,
            stack: config.stack,
            local_addr: SocketAddr::new(local_ip, listen_port),
            pending_conns: std::collections::VecDeque::new(),
        })
    }

    async fn connect_with(_config: DpdkConfig<S>) -> io::Result<S::Connection> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "DPDK does not support client-side connect; \
             use a regular TCP/UDP client",
        ))
    }
}

/// DPDK listener that busy-polls `rte_eth_rx_burst` and feeds frames
/// to the selected stack until a new connection is yielded.
pub struct DpdkListener<S: XdpStack> {
    _eal: Eal,
    port: DpdkPort,
    stack: S,
    local_addr: SocketAddr,
    pending_conns: std::collections::VecDeque<S::Connection>,
}

impl<S> LocalListener for DpdkListener<S>
where
    S: XdpStack + 'static,
    S::Connection: 'static,
    S::Error: std::fmt::Display + 'static,
{
    type Connection = S::Connection;
    type Error = io::Error;

    async fn accept(&mut self) -> io::Result<S::Connection> {
        loop {
            if let Some(conn) = self.pending_conns.pop_front() {
                return Ok(conn);
            }

            // rx burst
            let mut rx_pkts: [*mut ffi::rte_mbuf; MAX_BURST] =
                [std::ptr::null_mut(); MAX_BURST];
            // SAFETY: rx_pkts is a valid array of MAX_BURST pointers.
            let nb_rx = unsafe { self.port.rx_burst(&mut rx_pkts) };

            let mut tx_buf: Vec<Vec<u8>> = Vec::new();

            for i in 0..nb_rx as usize {
                let mbuf = rx_pkts[i];
                // SAFETY: mbuf was just returned by rte_eth_rx_burst
                // and is valid until we free it.
                let (data_ptr, data_len) = unsafe {
                    let ptr = ffi::mbuf_data_ptr(mbuf);
                    let len = ffi::rte_pktmbuf_data_len(mbuf);
                    (ptr, len as usize)
                };
                let frame = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };

                let mut q = FrameTxQueue::new(&mut tx_buf);
                if let Some(conn) = self
                    .stack
                    .on_rx(frame, &mut q)
                    .map_err(|e| io::Error::other(e.to_string()))?
                {
                    self.pending_conns.push_back(conn);
                }

                // Free the mbuf.
                // SAFETY: we are done reading this mbuf.
                unsafe {
                    ffi::rte_pktmbuf_free(mbuf);
                }
            }

            // tx burst: send any frames the stack produced.
            for frame_data in &tx_buf {
                // Allocate a fresh mbuf for each outbound frame.
                let mbuf = unsafe { ffi::rte_pktmbuf_alloc(self.port.pool()) };
                if mbuf.is_null() {
                    tracing::warn!("dpdk: rte_pktmbuf_alloc returned null, dropping tx frame");
                    continue;
                }
                // SAFETY: mbuf is freshly allocated and valid.
                let data_ptr = unsafe {
                    ffi::rte_pktmbuf_append(mbuf, frame_data.len() as u16)
                };
                if data_ptr.is_null() {
                    tracing::warn!("dpdk: rte_pktmbuf_append returned null (frame too large?)");
                    unsafe { ffi::rte_pktmbuf_free(mbuf); }
                    continue;
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        frame_data.as_ptr(),
                        data_ptr as *mut u8,
                        frame_data.len(),
                    );
                }
                let mut pkts = [mbuf];
                // SAFETY: pkts contains a valid mbuf.
                let sent = unsafe { self.port.tx_burst(&mut pkts, 1) };
                if sent == 0 {
                    // Free unsent mbuf.
                    unsafe { ffi::rte_pktmbuf_free(mbuf); }
                }
            }

            // Yield when idle.
            if nb_rx == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}
