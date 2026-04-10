//! AF_XDP datapath wrapper around `xsk-rs` (Linux + `xdp` feature only).
//!
//! This module owns the UMEM, the rx/tx/fill/completion queues and the
//! polling loop that drives an [`super::stack::XdpStack`] above the wire.
//!
//! # Safety
//!
//! `xsk-rs` requires `unsafe` to create a socket (shared UMEM semantics)
//! and to access UMEM-backed frame data.  All `unsafe` blocks in this
//! module are documented with `// SAFETY:` comments.

use crate::xdp::stack::{FrameTxQueue, XdpStack};
use std::io;
use xsk_rs::config::{Interface, SocketConfig, UmemConfig};
use xsk_rs::socket::{RxQueue, TxQueue};
use xsk_rs::umem::{CompQueue, FillQueue, Umem};
use xsk_rs::umem::frame::FrameDesc;

/// Configuration for the AF_XDP datapath.
#[derive(Debug, Clone)]
pub struct DatapathConfig {
    /// Interface name (`eth0`, `lo`, …).
    pub if_name: String,
    /// NIC queue id this datapath is bound to.
    pub queue_id: u32,
    /// Number of UMEM frames to allocate.  Must be a power of two.
    pub frame_count: u32,
    /// Per-frame size in bytes.  Must be a power of two ≥ 2048.
    pub frame_size: u32,
}

impl DatapathConfig {
    /// Creates a new datapath config with conservative defaults.
    #[must_use]
    pub fn new(if_name: impl Into<String>, queue_id: u32) -> Self {
        Self {
            if_name: if_name.into(),
            queue_id,
            frame_count: 4096,
            frame_size: 4096,
        }
    }

    /// Sets the number of UMEM frames.
    #[must_use]
    pub fn frame_count(mut self, count: u32) -> Self {
        self.frame_count = count;
        self
    }

    /// Sets the per-frame size.
    #[must_use]
    pub fn frame_size(mut self, size: u32) -> Self {
        self.frame_size = size;
        self
    }
}

/// AF_XDP datapath bound to a single `(interface, queue)` pair.
///
/// Holds the UMEM, the four ring queues, and the pool of frame
/// descriptors used to ferry packets between the kernel and userspace.
pub struct Datapath {
    umem: Umem,
    fill_q: FillQueue,
    comp_q: CompQueue,
    rx_q: RxQueue,
    tx_q: TxQueue,
    /// Pool of free frame descriptors that can be submitted to the fill
    /// queue (rx) or used for tx.  When a frame is consumed from the rx
    /// ring its descriptor moves here; when a frame is submitted to the
    /// tx ring its descriptor is taken from here.
    frame_descs: Vec<FrameDesc>,
}

impl Datapath {
    /// Binds an AF_XDP socket to the configured interface/queue.
    ///
    /// Requires `CAP_NET_ADMIN` and `CAP_BPF` (or root).
    ///
    /// # Errors
    /// Returns an `io::Error` if the kernel rejects the bind (missing
    /// capabilities, bad interface name, queue out of range, …).
    ///
    /// # Safety contract
    /// The caller guarantees that no other `Datapath` is bound to the
    /// same `(if_name, queue_id)` pair with the same `Umem`, or that
    /// the `XSK_LIBXDP_FLAGS_INHIBIT_PROG_LOAD` flag is set in the
    /// socket config if so.  In practice this means "one `Datapath`
    /// per queue per process".
    pub fn bind(config: &DatapathConfig) -> io::Result<Self> {
        let umem_config = UmemConfig::builder(
            config.frame_count.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "frame_count must be > 0",
                )
            })?,
            config.frame_size.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "frame_size must be > 0",
                )
            })?,
        )
        .build()
        .map_err(|e| io::Error::other(format!("umem config: {e}")))?;

        let (umem, mut frame_descs) = Umem::new(
            umem_config,
            config.frame_count.try_into().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "frame_count must be > 0")
            })?,
            false, // no huge pages
        )
        .map_err(|e| io::Error::other(format!("umem create: {e}")))?;

        let socket_config = SocketConfig::default();
        let interface = Interface::new(config.if_name.as_str())
            .map_err(|e| io::Error::other(format!("interface: {e}")))?;

        // SAFETY: We are the only Datapath binding to this
        // (if_name, queue_id) pair — the caller guarantees this per
        // the doc contract.
        let (tx_q, rx_q, fq_cq) = unsafe {
            xsk_rs::socket::Socket::new(
                socket_config,
                &umem,
                &interface,
                config.queue_id,
            )
        }
        .map_err(|e| io::Error::other(format!("xsk socket create: {e}")))?;

        let (mut fill_q, comp_q) = fq_cq.ok_or_else(|| {
            io::Error::other("xsk-rs did not return fill/completion queues")
        })?;

        // Submit half the frames to the fill queue so the kernel has
        // somewhere to write inbound packets.  Keep the other half for
        // tx.
        let half = frame_descs.len() / 2;
        let fill_frames: Vec<FrameDesc> = frame_descs.drain(..half).collect();

        // SAFETY: frame descriptors are valid UMEM offsets produced by
        // Umem::new and have not been submitted to any other queue.
        unsafe {
            fill_q.produce(&fill_frames);
        }

        Ok(Self {
            umem,
            fill_q,
            comp_q,
            rx_q,
            tx_q,
            frame_descs,
        })
    }

    /// Drives one poll round of the AF_XDP socket.
    ///
    /// 1. Reclaims completed tx descriptors from the completion queue.
    /// 2. Pulls inbound frames from the rx queue and hands them to the
    ///    stack via [`XdpStack::on_rx`].
    /// 3. Calls [`XdpStack::poll_timers`].
    /// 4. Submits any frames the stack produced into the tx queue.
    /// 5. Re-fills the fill queue with reclaimed descriptors.
    ///
    /// Returns the number of inbound frames processed.
    ///
    /// # Errors
    /// Returns an `io::Error` if any ring operation fails.
    pub fn poll_once<S: XdpStack>(&mut self, stack: &mut S) -> io::Result<usize>
    where
        S::Error: std::fmt::Display,
    {
        // 1. Reclaim completed tx descriptors so we can reuse them.
        let mut comp_descs = Vec::with_capacity(64);
        let n_comp = self.comp_q.consume(&mut comp_descs);
        for desc in comp_descs.iter().take(n_comp) {
            self.frame_descs.push(*desc);
        }

        // 2. Pull inbound frames from the rx ring.
        let mut rx_descs = Vec::with_capacity(64);
        let n_rx = self.rx_q.consume(&mut rx_descs);
        let mut tx_buf: Vec<Vec<u8>> = Vec::new();
        let mut processed = 0usize;

        for desc in rx_descs.iter().take(n_rx) {
            // SAFETY: the descriptor was just returned by the rx ring
            // and the backing UMEM slice is valid for the duration of
            // this loop iteration.  We copy the frame data out before
            // returning the descriptor to the fill queue.
            let data = unsafe { self.umem.data(desc) };
            let frame_bytes = data.contents();

            let mut q = FrameTxQueue::new(&mut tx_buf);
            stack
                .on_rx(frame_bytes, &mut q)
                .map_err(|e| io::Error::other(e.to_string()))?;
            processed += 1;
        }

        // Return rx descriptors to the fill queue so the kernel can
        // reuse them.
        if n_rx > 0 {
            // SAFETY: descriptors were consumed from the rx ring and
            // are no longer referenced by the kernel.
            unsafe {
                self.fill_q.produce(&rx_descs[..n_rx]);
            }
        }

        // 3. Let the stack flush timers.
        {
            let mut q = FrameTxQueue::new(&mut tx_buf);
            stack
                .poll_timers(&mut q)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }

        // 4. Submit pending tx frames.
        for frame_data in &tx_buf {
            if let Some(mut desc) = self.frame_descs.pop() {
                // SAFETY: `desc` is a valid free descriptor from our
                // pool, and `data_mut` gives exclusive access to its
                // UMEM-backed buffer.
                let data_mut = unsafe { self.umem.data_mut(&mut desc) };
                let buf = data_mut.contents_mut();
                let len = frame_data.len().min(buf.len());
                buf[..len].copy_from_slice(&frame_data[..len]);
                // Update the descriptor's data length.
                desc.lengths_mut().set_data(len);

                // SAFETY: we just wrote valid data into the descriptor's
                // UMEM region, and the descriptor has not been submitted
                // to any other queue.
                unsafe {
                    self.tx_q.produce_and_wakeup(&[desc])
                        .map_err(|e| io::Error::other(format!("tx produce: {e}")))?;
                }
            } else {
                tracing::warn!("xdp: no free tx descriptors, dropping outbound frame");
            }
        }

        // 5. Wakeup if needed (some drivers require explicit kicks).
        if self.tx_q.needs_wakeup() {
            self.tx_q.wakeup()?;
        }

        Ok(processed)
    }
}
