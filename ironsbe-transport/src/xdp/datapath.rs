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
use std::ffi::CString;
use std::io;
use std::io::Write;
use std::num::NonZeroU32;
use xsk_rs::config::{Interface, SocketConfig, UmemConfig};
use xsk_rs::socket::{RxQueue, TxQueue};
use xsk_rs::umem::frame::FrameDesc;
use xsk_rs::umem::{CompQueue, FillQueue, Umem};

/// Configuration for the AF_XDP datapath.
#[derive(Debug, Clone)]
pub struct DatapathConfig {
    /// Interface name (`eth0`, `lo`, …).
    pub if_name: String,
    /// NIC queue id this datapath is bound to.
    pub queue_id: u32,
    /// Number of UMEM frames to allocate.  Must be a power of two and
    /// non-zero.
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

/// Maximum number of descriptors to process per poll round.
const BATCH_SIZE: usize = 64;

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
    /// Pool of free frame descriptors.  Rx-consumed descriptors cycle
    /// back here after their contents have been read; tx descriptors
    /// are taken from here and returned via the completion queue.
    free_descs: Vec<FrameDesc>,
    /// Scratch space for rx consume calls (avoids re-allocating per
    /// poll round).
    rx_scratch: Vec<FrameDesc>,
    /// Scratch space for completion queue consume.
    comp_scratch: Vec<FrameDesc>,
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
    /// # Safety contract (upheld by the caller)
    /// No other `Datapath` may be bound to the same `(if_name, queue_id)`
    /// pair with a shared `Umem`, unless
    /// `XSK_LIBXDP_FLAGS_INHIBIT_PROG_LOAD` is set.  In practice:
    /// one `Datapath` per queue per process.
    pub fn bind(config: &DatapathConfig) -> io::Result<Self> {
        let frame_count = NonZeroU32::new(config.frame_count).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "frame_count must be > 0")
        })?;

        let umem_config = UmemConfig::builder()
            .build()
            .map_err(|e| io::Error::other(format!("umem config: {e}")))?;

        let (umem, mut frame_descs) = Umem::new(umem_config, frame_count, false)
            .map_err(|e| io::Error::other(format!("umem create: {e}")))?;

        let socket_config = SocketConfig::default();
        let if_cstr = CString::new(config.if_name.as_str())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let interface = Interface::new(if_cstr);

        // SAFETY: We are the only Datapath binding to this
        // (if_name, queue_id) pair — the caller guarantees this per
        // the doc contract above.
        let (tx_q, rx_q, fq_cq) = unsafe {
            xsk_rs::socket::Socket::new(socket_config, &umem, &interface, config.queue_id)
        }
        .map_err(|e| io::Error::other(format!("xsk socket create: {e}")))?;

        let (mut fill_q, comp_q) = fq_cq
            .ok_or_else(|| io::Error::other("xsk-rs did not return fill/completion queues"))?;

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
            free_descs: frame_descs,
            rx_scratch: vec![FrameDesc::default(); BATCH_SIZE],
            comp_scratch: vec![FrameDesc::default(); BATCH_SIZE],
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
    /// Returns a tuple of `(frames_processed, new_connections)`.  The
    /// caller should drain the connections vector to hand them to the
    /// application (e.g. via `LocalListener::accept`).
    ///
    /// # Errors
    /// Returns an `io::Error` if any ring operation fails.
    pub fn poll_once<S: XdpStack>(
        &mut self,
        stack: &mut S,
    ) -> io::Result<(usize, Vec<S::Connection>)>
    where
        S::Error: std::fmt::Display,
    {
        // 1. Reclaim completed tx descriptors.
        // SAFETY: comp_scratch is pre-allocated with valid default
        // FrameDescs; consume overwrites them with completed descs.
        let n_comp = unsafe { self.comp_q.consume(&mut self.comp_scratch) };
        for desc in self.comp_scratch.iter().take(n_comp) {
            self.free_descs.push(*desc);
        }

        // 2. Pull inbound frames from the rx ring.
        // SAFETY: rx_scratch is pre-allocated; consume overwrites with
        // received descriptors whose UMEM-backed data is valid until
        // we return them to the fill queue.
        let n_rx = unsafe { self.rx_q.consume(&mut self.rx_scratch) };
        let mut tx_buf: Vec<Vec<u8>> = Vec::new();
        let mut processed = 0usize;
        let mut new_conns: Vec<S::Connection> = Vec::new();

        for i in 0..n_rx {
            let desc = &self.rx_scratch[i];
            // SAFETY: descriptor was just returned by the rx ring;
            // the UMEM slice is valid until we return the desc to
            // the fill queue below.
            let data = unsafe { self.umem.data(desc) };
            let frame_bytes = data.contents();

            let mut q = FrameTxQueue::new(&mut tx_buf);
            if let Some(conn) = stack
                .on_rx(frame_bytes, &mut q)
                .map_err(|e| io::Error::other(e.to_string()))?
            {
                new_conns.push(conn);
            }
            processed += 1;
        }

        // Return rx descriptors to the fill queue for kernel reuse.
        if n_rx > 0 {
            // SAFETY: descriptors were consumed from rx and are no
            // longer referenced.
            unsafe {
                self.fill_q.produce(&self.rx_scratch[..n_rx]);
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
            if let Some(mut desc) = self.free_descs.pop() {
                // Write the frame data into the descriptor's UMEM
                // region via a scoped block so the mutable borrow on
                // `desc` ends before we hand it to the tx queue.
                {
                    // SAFETY: `desc` is a valid free descriptor from
                    // our pool; `data_mut` gives exclusive UMEM access.
                    let mut data_mut = unsafe { self.umem.data_mut(&mut desc) };
                    // `cursor()` sets the data length as we write.
                    let mut cursor = data_mut.cursor();
                    if let Err(e) = cursor.write_all(frame_data) {
                        tracing::warn!("xdp: tx write failed: {e}");
                        self.free_descs.push(desc);
                        continue;
                    }
                }

                // SAFETY: we just wrote valid data into the
                // descriptor's UMEM region.
                unsafe {
                    self.tx_q
                        .produce_and_wakeup(&[desc])
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

        Ok((processed, new_conns))
    }
}
