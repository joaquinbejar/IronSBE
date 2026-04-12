//! RDMA connection wrapping an ibverbs Queue Pair (QP).
//!
//! The [`RdmaConnection`] takes ownership of the Protection Domain
//! and Completion Queue that were used to create the QP on an
//! accepted `rdma_cm_id`.  This guarantees the CQ the connection
//! polls matches the CQ the QP actually references, and that PD/CQ
//! live at least as long as the QP.
//!
//! The SBE framing is the same as the other backends: each message
//! is a 4-byte little-endian length prefix followed by the payload.
//!
//! ## Completion draining
//!
//! The CQ is shared between SEND and RECV work requests.  Every
//! SEND is posted signaled (so rdma-core reports failures), which
//! means each SEND consumes a CQ slot until it is drained.  Prior
//! to #39 the CQ was only drained opportunistically inside
//! [`RdmaConnection::recv`], so a burst of `send()` calls without
//! matching `recv()`s would fill the CQ and push the QP into error
//! state.  Now:
//!
//! - [`RdmaConnection::send`] drains pending SEND completions before
//!   posting a new WR, preventing CQ overflow.
//! - During a send-side drain, any RECV completion the driver has
//!   already produced is preserved in [`Self::pending_recvs`] and
//!   delivered by the next [`RdmaConnection::recv`] call, so no
//!   bytes are ever dropped.

use crate::ffi;
use crate::listener::{
    BorrowedEventFd, cleanup_accept_resources, create_qp_resources, extract_dst_addr,
    wait_for_cm_event,
};
use bytes::BytesMut;
use ironsbe_transport::traits::LocalConnection;
use std::collections::VecDeque;
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::os::fd::RawFd;
use std::ptr;
use std::rc::Rc;
use tokio::io::unix::AsyncFd;

/// Length prefix size (matches all other IronSBE backends).
const LENGTH_PREFIX_BYTES: usize = 4;

/// Number of pre-posted RECV work requests.
const RECV_DEPTH: usize = 16;

/// Completion queue capacity (set when creating the CQ in
/// [`crate::listener`]).  `pub(crate)` so the listener can share
/// the same constant instead of hard-coding `32i32`.
pub(crate) const CQ_CAPACITY: u32 = 32;

/// Minimum CQ headroom reserved for RECV completions while
/// throttling pending SEND completions.  Derived from `RECV_DEPTH`
/// so the CQ always retains enough space for the maximum number of
/// outstanding RECV work requests.
const CQ_RECV_HEADROOM: u32 = RECV_DEPTH as u32;

/// Drain SEND completions inside `send` once the pending count
/// reaches this threshold.  Derived from `CQ_CAPACITY` and
/// `RECV_DEPTH` so a concurrent burst of RECV completions can never
/// overflow the CQ.
const CQ_DRAIN_HIGH_WATER: u32 = CQ_CAPACITY - CQ_RECV_HEADROOM;

/// Stop draining once the pending count falls below this threshold.
/// The hysteresis avoids draining for a single slot when the CQ is
/// close to the high-water mark.
const CQ_DRAIN_LOW_WATER: u32 = CQ_DRAIN_HIGH_WATER / 2;

// Compile-time invariants for the CQ draining watermarks.
const _: () = assert!(CQ_RECV_HEADROOM <= CQ_CAPACITY);
const _: () = assert!(CQ_DRAIN_LOW_WATER < CQ_DRAIN_HIGH_WATER);
const _: () = assert!(CQ_DRAIN_HIGH_WATER < CQ_CAPACITY);
const _: () = assert!(CQ_CAPACITY - CQ_DRAIN_HIGH_WATER >= CQ_RECV_HEADROOM);

/// A RECV completion that was observed while the `send` path was
/// draining the CQ.  Stored in [`RdmaConnection::pending_recvs`] so
/// the next [`RdmaConnection::recv`] call can return its bytes
/// without re-polling.
#[derive(Clone, Copy, Debug)]
struct PendingRecv {
    buf_idx: usize,
    byte_len: u32,
}

/// Outcome of a single `drain_cq` step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drained {
    /// The CQ was empty — nothing to do.
    Empty,
    /// A SEND completion was drained (pending_sends was decremented).
    Send,
    /// A RECV completion was buffered in `pending_recvs`.
    Recv,
    /// Some other opcode — logged and ignored.
    Other,
}

/// An established RDMA connection.
///
/// The `PhantomData<Rc<()>>` marker makes this `!Send` because all
/// RDMA operations (post_send, post_recv, poll_cq) are thread-bound
/// to the thread that created the QP/CQ in practice.
pub struct RdmaConnection {
    /// CM ID with an attached QP that references `pd` and `cq`.
    cm_id: *mut ffi::rdma_cm_id,
    /// Protection Domain used for the QP and all MRs.
    pd: *mut ffi::ibv_pd,
    /// Completion Queue bound to the QP.
    cq: *mut ffi::ibv_cq,
    /// Pre-registered send buffer (length prefix + payload).
    send_buf: Vec<u8>,
    send_mr: *mut ffi::ibv_mr,
    /// Pre-registered receive buffers.
    recv_bufs: Vec<Vec<u8>>,
    recv_mrs: Vec<*mut ffi::ibv_mr>,
    max_msg_size: usize,
    peer_addr: SocketAddr,
    /// Signaled SEND WRs posted but not yet drained from the CQ.
    /// Bounded by `CQ_DRAIN_HIGH_WATER`, which is well below
    /// `CQ_CAPACITY`, so a concurrent burst of RECV completions
    /// cannot push the CQ over capacity.
    pending_sends: u32,
    /// RECV completions observed while draining the CQ from the
    /// `send` path.  The next `recv` call pops from the head of this
    /// queue before polling the CQ directly.
    pending_recvs: VecDeque<PendingRecv>,
    /// Owned event channel for client-initiated connections.  `None`
    /// for server-side (accepted) connections — the listener owns
    /// theirs.  Cleaned up in [`Drop`] after `rdma_destroy_id`.
    event_channel: Option<*mut ffi::rdma_event_channel>,
    /// Makes this type `!Send` + `!Sync`.
    _not_send: PhantomData<Rc<()>>,
}

impl RdmaConnection {
    /// Creates a connection from an already-accepted CM ID plus the
    /// PD and CQ that were used to create its QP.
    ///
    /// On any error, cleans up all partially-allocated resources
    /// (MRs, but **not** the caller-provided PD/CQ/cm_id — the
    /// caller still owns those until a successful return).
    ///
    /// # Safety
    /// `cm_id` must be a valid, connected `rdma_cm_id` whose QP was
    /// created against `pd` and `cq`.  Ownership of all three
    /// transfers to the returned connection on success; on failure
    /// the caller is responsible for releasing them.
    pub(crate) unsafe fn from_accepted_cm_id(
        cm_id: *mut ffi::rdma_cm_id,
        pd: *mut ffi::ibv_pd,
        cq: *mut ffi::ibv_cq,
        peer_addr: SocketAddr,
        max_msg_size: usize,
    ) -> io::Result<Self> {
        let buf_size = LENGTH_PREFIX_BYTES + max_msg_size;

        // Register send buffer — LOCAL_WRITE only.  Two-sided SEND
        // does not need the remote peer to write into our buffer.
        let mut send_buf = vec![0u8; buf_size];
        let send_mr = unsafe {
            ffi::ibv_reg_mr(
                pd,
                send_buf.as_mut_ptr().cast(),
                buf_size,
                ffi::IBV_ACCESS_LOCAL_WRITE as core::ffi::c_int,
            )
        };
        if send_mr.is_null() {
            return Err(io::Error::other("ibv_reg_mr (send) failed"));
        }

        // Register receive buffers.
        let mut recv_bufs: Vec<Vec<u8>> = Vec::with_capacity(RECV_DEPTH);
        let mut recv_mrs: Vec<*mut ffi::ibv_mr> = Vec::with_capacity(RECV_DEPTH);
        let mut setup_error: Option<io::Error> = None;
        for _ in 0..RECV_DEPTH {
            let mut buf = vec![0u8; buf_size];
            let mr = unsafe {
                ffi::ibv_reg_mr(
                    pd,
                    buf.as_mut_ptr().cast(),
                    buf_size,
                    ffi::IBV_ACCESS_LOCAL_WRITE as core::ffi::c_int,
                )
            };
            if mr.is_null() {
                setup_error = Some(io::Error::other("ibv_reg_mr (recv) failed"));
                break;
            }
            recv_bufs.push(buf);
            recv_mrs.push(mr);
        }

        if let Some(err) = setup_error {
            // Clean up our partially-allocated MRs.
            unsafe {
                ffi::ibv_dereg_mr(send_mr);
                for mr in &recv_mrs {
                    ffi::ibv_dereg_mr(*mr);
                }
            }
            return Err(err);
        }

        let conn = Self {
            cm_id,
            pd,
            cq,
            send_buf,
            send_mr,
            recv_bufs,
            recv_mrs,
            max_msg_size,
            peer_addr,
            pending_sends: 0,
            event_channel: None,
            pending_recvs: VecDeque::with_capacity(RECV_DEPTH),
            _not_send: PhantomData,
        };

        // Pre-post all receive buffers.  If any post fails, the
        // connection's Drop will clean up everything.
        for i in 0..RECV_DEPTH {
            conn.post_recv(i)?;
        }

        Ok(conn)
    }

    /// Establishes a client-side RDMA connection to `addr`.
    ///
    /// Drives the full RDMA CM client handshake:
    /// `resolve_addr` → `resolve_route` → `create QP` → `connect`,
    /// waiting for each CM event via
    /// [`tokio::io::unix::AsyncFd`] so the runtime is never blocked.
    ///
    /// The event channel created for the handshake is kept alive for
    /// the lifetime of the connection (rdma-core requires it) and
    /// cleaned up in [`Drop`].
    ///
    /// # Errors
    /// Returns `io::Error` if any RDMA CM step fails or if the
    /// event-channel fd cannot be registered with the tokio reactor.
    pub async fn connect(addr: SocketAddr, max_msg_size: usize) -> io::Result<Self> {
        /// Timeout in milliseconds for `rdma_resolve_addr` /
        /// `rdma_resolve_route`.
        const RESOLVE_TIMEOUT_MS: i32 = 5000;

        // --- event channel + CM ID ----------------------------------
        let ec = unsafe { ffi::rdma_create_event_channel() };
        if ec.is_null() {
            return Err(io::Error::other("rdma_create_event_channel failed"));
        }

        let event_fd: RawFd = unsafe { (*ec).fd };
        let flags = unsafe { libc::fcntl(event_fd, libc::F_GETFL) };
        if flags < 0 {
            unsafe { ffi::rdma_destroy_event_channel(ec) };
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(event_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let err = io::Error::last_os_error();
            unsafe { ffi::rdma_destroy_event_channel(ec) };
            return Err(err);
        }

        // Wrapped in Option so cleanup macros can drop it BEFORE
        // destroying the event channel fd, avoiding the stale-fd
        // deregistration race (same pattern as RdmaListener::Drop).
        let mut async_fd = Some(match AsyncFd::new(BorrowedEventFd(event_fd)) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe { ffi::rdma_destroy_event_channel(ec) };
                return Err(e);
            }
        });

        let mut cm_id: *mut ffi::rdma_cm_id = ptr::null_mut();
        let ret = unsafe {
            ffi::rdma_create_id(
                ec,
                &mut cm_id,
                ptr::null_mut(),
                ffi::rdma_port_space_RDMA_PS_TCP,
            )
        };
        if ret != 0 {
            drop(async_fd);
            unsafe { ffi::rdma_destroy_event_channel(ec) };
            return Err(io::Error::other(format!("rdma_create_id failed: {ret}")));
        }

        // --- resolve address ----------------------------------------
        let dst_sockaddr = match addr {
            SocketAddr::V4(v4) => {
                let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                sa.sin_family = libc::AF_INET as u16;
                sa.sin_port = v4.port().to_be();
                sa.sin_addr.s_addr = u32::from_ne_bytes(v4.ip().octets());
                sa
            }
            SocketAddr::V6(_) => {
                drop(async_fd);
                unsafe {
                    ffi::rdma_destroy_id(cm_id);
                    ffi::rdma_destroy_event_channel(ec);
                }
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "IPv6 not supported for RDMA connect",
                ));
            }
        };

        // Cleanup helper for the pre-QP phase: only cm_id + ec exist.
        // Drops async_fd BEFORE destroying the event channel to avoid
        // the stale-fd deregistration race.
        macro_rules! cleanup_pre_qp {
            () => {
                drop(async_fd.take());
                unsafe {
                    ffi::rdma_destroy_id(cm_id);
                    ffi::rdma_destroy_event_channel(ec);
                }
            };
        }

        tracing::debug!("rdma_resolve_addr → {addr}");
        let ret = unsafe {
            ffi::rdma_resolve_addr(
                cm_id,
                ptr::null_mut(), // src: let the kernel pick
                &dst_sockaddr as *const libc::sockaddr_in as *mut ffi::sockaddr,
                RESOLVE_TIMEOUT_MS,
            )
        };
        if ret != 0 {
            cleanup_pre_qp!();
            return Err(io::Error::other(format!("rdma_resolve_addr failed: {ret}")));
        }
        tracing::debug!("waiting for ADDR_RESOLVED");

        if let Err(e) = wait_for_cm_event(
            async_fd.as_ref().expect("async_fd taken during cleanup"),
            ec,
            ffi::rdma_cm_event_type_RDMA_CM_EVENT_ADDR_RESOLVED,
        )
        .await
        {
            cleanup_pre_qp!();
            return Err(io::Error::other(format!("waiting for ADDR_RESOLVED: {e}")));
        }
        tracing::debug!("ADDR_RESOLVED received");

        // --- resolve route ------------------------------------------
        let ret = unsafe { ffi::rdma_resolve_route(cm_id, RESOLVE_TIMEOUT_MS) };
        if ret != 0 {
            cleanup_pre_qp!();
            return Err(io::Error::other(format!(
                "rdma_resolve_route failed: {ret}"
            )));
        }
        tracing::debug!("waiting for ROUTE_RESOLVED");

        if let Err(e) = wait_for_cm_event(
            async_fd.as_ref().expect("async_fd taken during cleanup"),
            ec,
            ffi::rdma_cm_event_type_RDMA_CM_EVENT_ROUTE_RESOLVED,
        )
        .await
        {
            cleanup_pre_qp!();
            return Err(io::Error::other(format!("waiting for ROUTE_RESOLVED: {e}")));
        }
        tracing::debug!("ROUTE_RESOLVED received, creating QP resources");

        // --- create PD + CQ + QP -----------------------------------
        let (pd, cq) = match unsafe { create_qp_resources(cm_id) } {
            Ok(res) => res,
            Err(e) => {
                cleanup_pre_qp!();
                return Err(e);
            }
        };
        tracing::debug!("QP resources created, connecting");

        // From here on, QP resources exist and must be torn down
        // via `cleanup_accept_resources` on failure.
        macro_rules! cleanup_post_qp {
            () => {
                drop(async_fd.take());
                unsafe { cleanup_accept_resources(cm_id, pd, cq, true) };
                unsafe { ffi::rdma_destroy_event_channel(ec) };
            };
        }

        // --- connect ------------------------------------------------
        let mut conn_param: ffi::rdma_conn_param = unsafe { std::mem::zeroed() };
        conn_param.initiator_depth = 1;
        conn_param.responder_resources = 1;
        let ret = unsafe { ffi::rdma_connect(cm_id, &mut conn_param) };
        if ret != 0 {
            cleanup_post_qp!();
            return Err(io::Error::other(format!("rdma_connect failed: {ret}")));
        }

        // Yield to the runtime so the reactor can poll epoll and
        // deliver the CONNECT_REQUEST edge-trigger to the server's
        // accept future before we enter our own wait.  Without this,
        // both futures go Pending in the same tick and the
        // edge-triggered notification from rdma_connect may not
        // propagate to the server's AsyncFd on SoftRoCE.
        tokio::task::yield_now().await;

        if let Err(e) = wait_for_cm_event(
            async_fd.as_ref().expect("async_fd taken during cleanup"),
            ec,
            ffi::rdma_cm_event_type_RDMA_CM_EVENT_ESTABLISHED,
        )
        .await
        {
            cleanup_post_qp!();
            return Err(io::Error::other(format!("waiting for ESTABLISHED: {e}")));
        }

        // AsyncFd drops naturally at function exit; the fd stays open
        // via the event channel that we transfer to the connection.
        let peer_addr = unsafe { extract_dst_addr(cm_id) }.unwrap_or(addr);

        tracing::info!(%addr, %peer_addr, "RDMA client connected");

        let mut conn =
            match unsafe { Self::from_accepted_cm_id(cm_id, pd, cq, peer_addr, max_msg_size) } {
                Ok(c) => c,
                Err(e) => {
                    cleanup_post_qp!();
                    return Err(e);
                }
            };
        conn.event_channel = Some(ec);
        Ok(conn)
    }

    /// Posts one RECV work request for buffer at index `idx`.
    fn post_recv(&self, idx: usize) -> io::Result<()> {
        let buf_size = LENGTH_PREFIX_BYTES + self.max_msg_size;
        let mr = self.recv_mrs[idx];
        let mut sge = ffi::ibv_sge {
            addr: self.recv_bufs[idx].as_ptr() as u64,
            length: buf_size as u32,
            lkey: unsafe { (*mr).lkey },
        };
        let mut wr: ffi::ibv_recv_wr = unsafe { std::mem::zeroed() };
        wr.wr_id = idx as u64;
        wr.sg_list = &mut sge;
        wr.num_sge = 1;

        let mut bad_wr: *mut ffi::ibv_recv_wr = ptr::null_mut();
        let qp = unsafe { (*self.cm_id).qp };
        let ret = unsafe { ffi::ironsbe_ibv_post_recv(qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            return Err(io::Error::other(format!("ibv_post_recv failed: {ret}")));
        }
        Ok(())
    }

    /// Drains a single work completion from the CQ, classifying it
    /// and updating internal state.
    ///
    /// - SEND completions decrement [`Self::pending_sends`].
    /// - RECV completions push a [`PendingRecv`] onto
    ///   [`Self::pending_recvs`] for the next `recv` call to pick up.
    /// - An empty CQ returns [`Drained::Empty`] with no side effects.
    /// - Non-success work completions return an `Err`, preserving the
    ///   existing error surface.
    fn drain_cq(&mut self) -> io::Result<Drained> {
        let mut wc: ffi::ibv_wc = unsafe { std::mem::zeroed() };
        let n = unsafe { ffi::ironsbe_ibv_poll_cq(self.cq, 1, &mut wc) };
        if n < 0 {
            return Err(io::Error::other("ibv_poll_cq failed"));
        }
        if n == 0 {
            return Ok(Drained::Empty);
        }
        if wc.status != ffi::ibv_wc_status_IBV_WC_SUCCESS {
            return Err(io::Error::other(format!(
                "RDMA work completion error: status={}",
                wc.status
            )));
        }
        if wc.opcode == ffi::ibv_wc_opcode_IBV_WC_RECV {
            let pending = PendingRecv {
                buf_idx: wc.wr_id as usize,
                byte_len: wc.byte_len,
            };
            self.pending_recvs.push_back(pending);
            Ok(Drained::Recv)
        } else if wc.opcode == ffi::ibv_wc_opcode_IBV_WC_SEND {
            // Saturate at zero so a stray signaled SEND never
            // underflows the counter (e.g. if the peer disconnected
            // between the drain and the next send).
            self.pending_sends = self.pending_sends.saturating_sub(1);
            Ok(Drained::Send)
        } else {
            tracing::warn!(opcode = wc.opcode, "unexpected RDMA work completion opcode");
            Ok(Drained::Other)
        }
    }

    /// Consumes a buffered [`PendingRecv`] (if any) and turns it
    /// into an `Option<BytesMut>` using the same framing rules as
    /// the live-CQ path.  Re-posts the receive buffer on the way out.
    fn consume_pending_recv(&mut self, pending: PendingRecv) -> io::Result<Option<BytesMut>> {
        let idx = pending.buf_idx;
        let byte_len = pending.byte_len as usize;

        if byte_len < LENGTH_PREFIX_BYTES {
            self.post_recv(idx)?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed RDMA frame: byte_len {byte_len} < prefix {LENGTH_PREFIX_BYTES}"),
            ));
        }

        let buf = &self.recv_bufs[idx];
        let msg_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let total = LENGTH_PREFIX_BYTES + msg_len;
        if msg_len > self.max_msg_size {
            self.post_recv(idx)?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "oversized RDMA frame: msg_len {msg_len} > max_msg_size {}",
                    self.max_msg_size
                ),
            ));
        }
        if total > byte_len {
            self.post_recv(idx)?;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("truncated RDMA frame: declared {total} bytes, got {byte_len}"),
            ));
        }
        let payload = BytesMut::from(&buf[LENGTH_PREFIX_BYTES..total]);
        self.post_recv(idx)?;
        Ok(Some(payload))
    }
}

impl LocalConnection for RdmaConnection {
    type Error = io::Error;

    async fn recv(&mut self) -> io::Result<Option<BytesMut>> {
        // First deliver anything buffered during a send-side drain.
        if let Some(pending) = self.pending_recvs.pop_front() {
            return self.consume_pending_recv(pending);
        }

        loop {
            match self.drain_cq()? {
                Drained::Empty => {
                    tokio::task::yield_now().await;
                    continue;
                }
                Drained::Recv => {
                    if let Some(pending) = self.pending_recvs.pop_front() {
                        return self.consume_pending_recv(pending);
                    }
                }
                Drained::Send | Drained::Other => {
                    // Keep looping — we need a RECV for this call.
                }
            }
        }
    }

    async fn send(&mut self, msg: &[u8]) -> io::Result<()> {
        if msg.len() > self.max_msg_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "message length {} exceeds max_msg_size {}",
                    msg.len(),
                    self.max_msg_size
                ),
            ));
        }

        // Drain pending SEND completions before posting.  This
        // keeps `pending_sends` well below `CQ_CAPACITY` even when
        // the caller bursts sends back-to-back with no interleaved
        // recvs.  See #39.
        if self.pending_sends >= CQ_DRAIN_HIGH_WATER {
            self.drain_until_low_water().await?;
        }

        let frame_len = u32::try_from(msg.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("message length {} exceeds u32::MAX", msg.len()),
            )
        })?;
        self.send_buf[..LENGTH_PREFIX_BYTES].copy_from_slice(&frame_len.to_le_bytes());
        self.send_buf[LENGTH_PREFIX_BYTES..LENGTH_PREFIX_BYTES + msg.len()].copy_from_slice(msg);
        let total = LENGTH_PREFIX_BYTES.checked_add(msg.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "framed message length overflow")
        })?;
        let total_u32 = u32::try_from(total).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("framed length {total} exceeds u32::MAX"),
            )
        })?;

        let mut sge = ffi::ibv_sge {
            addr: self.send_buf.as_ptr() as u64,
            length: total_u32,
            lkey: unsafe { (*self.send_mr).lkey },
        };
        let mut wr: ffi::ibv_send_wr = unsafe { std::mem::zeroed() };
        wr.sg_list = &mut sge;
        wr.num_sge = 1;
        wr.opcode = ffi::ibv_wr_opcode_IBV_WR_SEND;
        wr.send_flags = ffi::IBV_SEND_SIGNALED;

        let mut bad_wr: *mut ffi::ibv_send_wr = ptr::null_mut();
        let qp = unsafe { (*self.cm_id).qp };
        let ret = unsafe { ffi::ironsbe_ibv_post_send(qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            return Err(io::Error::other(format!("ibv_post_send failed: {ret}")));
        }

        // Track the signaled WR so the next send knows how close we
        // are to the CQ ceiling.  `checked_add` guards the (in
        // practice unreachable) `u32::MAX` overflow because
        // `pending_sends` is bounded by `CQ_CAPACITY` in steady state.
        self.pending_sends = self
            .pending_sends
            .checked_add(1)
            .ok_or_else(|| io::Error::other("pending_sends counter overflow"))?;
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }
}

impl RdmaConnection {
    /// Drains the CQ until `pending_sends` falls below the
    /// low-water mark.  If the CQ is transiently empty (completions
    /// haven't arrived from the NIC yet) we yield to the runtime
    /// and retry, mirroring the `recv` loop's behaviour.  This
    /// avoids a spurious `WouldBlock` error that would surface under
    /// normal back-to-back send load.
    async fn drain_until_low_water(&mut self) -> io::Result<()> {
        while self.pending_sends >= CQ_DRAIN_LOW_WATER {
            match self.drain_cq()? {
                Drained::Empty => {
                    // Completions may not have arrived from the NIC
                    // yet.  A short sleep (rather than a bare
                    // yield_now) gives the kernel's SoftRoCE / NIC
                    // driver time to produce CQ entries before we
                    // re-poll.
                    tokio::time::sleep(std::time::Duration::from_micros(10)).await;
                }
                // SEND, RECV, Other all count as progress — the
                // loop predicate (`pending_sends >= low_water`) is
                // what gates the exit.
                Drained::Send | Drained::Recv | Drained::Other => {}
            }
        }
        Ok(())
    }
}

impl Drop for RdmaConnection {
    fn drop(&mut self) {
        unsafe {
            if !self.send_mr.is_null() {
                ffi::ibv_dereg_mr(self.send_mr);
            }
            for mr in &self.recv_mrs {
                if !mr.is_null() {
                    ffi::ibv_dereg_mr(*mr);
                }
            }
            if !self.cm_id.is_null() {
                ffi::rdma_disconnect(self.cm_id);
                ffi::rdma_destroy_qp(self.cm_id);
                ffi::rdma_destroy_id(self.cm_id);
            }
            if !self.cq.is_null() {
                ffi::ibv_destroy_cq(self.cq);
            }
            if !self.pd.is_null() {
                ffi::ibv_dealloc_pd(self.pd);
            }
            // Client-initiated connections own their event channel;
            // destroy it after the cm_id is gone.
            if let Some(ec) = self.event_channel
                && !ec.is_null()
            {
                ffi::rdma_destroy_event_channel(ec);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pending-recv inbox is a FIFO so the next `recv()` call
    /// returns completions in the order the driver produced them.
    /// Guards against an accidental `VecDeque` → `Vec` regression.
    /// See #39.
    #[test]
    fn test_pending_recv_fifo_ordering() {
        let mut q: VecDeque<PendingRecv> = VecDeque::new();
        q.push_back(PendingRecv {
            buf_idx: 3,
            byte_len: 10,
        });
        q.push_back(PendingRecv {
            buf_idx: 1,
            byte_len: 20,
        });
        q.push_back(PendingRecv {
            buf_idx: 5,
            byte_len: 30,
        });

        let first = q.pop_front().expect("first");
        assert_eq!(first.buf_idx, 3);
        assert_eq!(first.byte_len, 10);

        let second = q.pop_front().expect("second");
        assert_eq!(second.buf_idx, 1);

        let third = q.pop_front().expect("third");
        assert_eq!(third.buf_idx, 5);

        assert!(q.pop_front().is_none());
    }
}
