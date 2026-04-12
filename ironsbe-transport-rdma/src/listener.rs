//! RDMA CM listener wrapping `rdma_cm_id` in listening mode.
//!
//! The listener integrates with tokio's runtime via
//! [`tokio::io::unix::AsyncFd`] on the `rdma_event_channel`'s file
//! descriptor, so [`RdmaListener::accept`] never blocks a worker
//! thread.  Both `local_addr()` and the accepted connection's
//! `peer_addr()` report real endpoints extracted from the
//! underlying `rdma_cm_id` route information — so a bind to
//! `0.0.0.0:0` surfaces a concrete IP/port and the connection
//! reports the remote side of the link, not the listener's bind
//! address.  See #39.

use crate::addr::sockaddr_to_socket_addr;
use crate::connection::RdmaConnection;
use crate::ffi;
use ironsbe_transport::traits::LocalListener;
use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, RawFd};
use std::ptr;
use tokio::io::unix::AsyncFd;

/// Default backlog for `rdma_listen`.
const LISTEN_BACKLOG: i32 = 16;

/// Borrowed file descriptor wrapper with **no** `Drop` impl.
///
/// [`AsyncFd`] registers its `T` with the tokio reactor and
/// deregisters on drop, but delegates ownership of the underlying
/// fd to `T` — meaning if `T: Drop` closes the fd, tokio honours
/// that.  The `rdma_event_channel`'s fd is owned by rdma-core and
/// must only be closed by `rdma_destroy_event_channel`, so this
/// wrapper deliberately holds only a [`RawFd`] and has no `Drop` to
/// avoid accidentally closing it.
pub(crate) struct BorrowedEventFd(pub(crate) RawFd);

impl AsRawFd for BorrowedEventFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

/// RDMA CM listener.
///
/// Wraps a `rdma_cm_id` in listening mode.  `accept()` waits for an
/// incoming RDMA CM connection request, creates a QP + PD + CQ on
/// the new CM ID, hands them to [`RdmaConnection`] for ownership,
/// and returns the new connection.
pub struct RdmaListener {
    listen_id: *mut ffi::rdma_cm_id,
    event_channel: *mut ffi::rdma_event_channel,
    /// Registered with the tokio reactor so `accept` awaits event
    /// readiness instead of blocking on `rdma_get_cm_event`.
    ///
    /// Wrapped in `Option` so our `Drop` impl can deregister it from
    /// epoll *before* `rdma_destroy_event_channel` closes the
    /// underlying fd.  Without this explicit ordering, another
    /// thread could theoretically reuse the same numeric fd between
    /// the channel destruction and the `AsyncFd` drop, causing tokio
    /// to deregister the wrong fd.  See #39.
    async_fd: Option<AsyncFd<BorrowedEventFd>>,
    local_addr: SocketAddr,
    max_msg_size: usize,
}

impl RdmaListener {
    /// Binds and listens on `addr`.
    ///
    /// On any error, releases partially-allocated resources before
    /// returning.  After `rdma_listen` succeeds the effective bound
    /// address is read back from the listen CM ID via
    /// [`sockaddr_to_socket_addr`] and stored — so a bind to
    /// `0.0.0.0:0` reports a concrete IP/port via [`Self::local_addr`].
    ///
    /// # Errors
    /// Returns an `io::Error` if any RDMA CM call fails or if the
    /// event-channel fd cannot be registered with the tokio reactor.
    pub fn bind(addr: SocketAddr, max_msg_size: usize) -> io::Result<Self> {
        let ec = unsafe { ffi::rdma_create_event_channel() };
        if ec.is_null() {
            return Err(io::Error::other("rdma_create_event_channel failed"));
        }

        // Pull out the raw fd from the event channel so we can mark
        // it non-blocking and hand it to the tokio reactor.  The
        // struct's layout is `{ fd: c_int }` and bindgen exposes
        // `fd` as a direct field.
        let event_fd: RawFd = unsafe { (*ec).fd };

        // Set O_NONBLOCK: without this, `rdma_get_cm_event` will
        // still block inside the read() syscall even if the fd is
        // epoll-ready.  With it, `rdma_get_cm_event` returns -1 /
        // EAGAIN when the channel is empty, which is what the
        // AsyncFd loop expects.
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

        let mut listen_id: *mut ffi::rdma_cm_id = ptr::null_mut();
        let ret = unsafe {
            ffi::rdma_create_id(
                ec,
                &mut listen_id,
                ptr::null_mut(),
                ffi::rdma_port_space_RDMA_PS_TCP,
            )
        };
        if ret != 0 {
            unsafe { ffi::rdma_destroy_event_channel(ec) };
            return Err(io::Error::other(format!("rdma_create_id failed: {ret}")));
        }

        // Convert SocketAddr to sockaddr_in.  `sin_addr.s_addr` must
        // be in network byte order; `from_ne_bytes` on the IP octets
        // gives exactly that (octets are already big-endian).
        let sockaddr = match addr {
            SocketAddr::V4(v4) => {
                let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                sa.sin_family = libc::AF_INET as u16;
                sa.sin_port = v4.port().to_be();
                sa.sin_addr.s_addr = u32::from_ne_bytes(v4.ip().octets());
                sa
            }
            SocketAddr::V6(_) => {
                unsafe {
                    ffi::rdma_destroy_id(listen_id);
                    ffi::rdma_destroy_event_channel(ec);
                }
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "IPv6 not supported for RDMA listener",
                ));
            }
        };

        let ret = unsafe {
            ffi::rdma_bind_addr(
                listen_id,
                &sockaddr as *const libc::sockaddr_in as *mut ffi::sockaddr,
            )
        };
        if ret != 0 {
            unsafe {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(ec);
            }
            return Err(io::Error::other(format!("rdma_bind_addr failed: {ret}")));
        }

        let ret = unsafe { ffi::rdma_listen(listen_id, LISTEN_BACKLOG) };
        if ret != 0 {
            unsafe {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(ec);
            }
            return Err(io::Error::other(format!("rdma_listen failed: {ret}")));
        }

        // Extract the real bound address from the listen id after
        // rdma_listen — for a bind to 0.0.0.0:0 this is where the
        // OS-assigned port and concrete IP come from.  See #39.
        let local_addr = match unsafe { extract_src_addr(listen_id) } {
            Ok(real) => real,
            Err(e) => {
                // Fall back to the requested address with a warning:
                // returning an error here would regress the existing
                // behaviour for callers that bound to a concrete
                // address.
                tracing::warn!(
                    error = %e,
                    "could not extract bound address from listen id, falling back to requested bind_addr"
                );
                addr
            }
        };

        // Register the event-channel fd with the tokio reactor.
        // BorrowedEventFd has no Drop, so tokio will deregister from
        // epoll but won't close the fd — rdma_destroy_event_channel
        // is still responsible for that in our Drop impl.
        let async_fd = match AsyncFd::new(BorrowedEventFd(event_fd)) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    ffi::rdma_destroy_id(listen_id);
                    ffi::rdma_destroy_event_channel(ec);
                }
                return Err(e);
            }
        };

        tracing::info!(%local_addr, "RDMA listener bound");

        Ok(Self {
            listen_id,
            event_channel: ec,
            async_fd: Some(async_fd),
            local_addr,
            max_msg_size,
        })
    }
}

/// Extracts the source address stored on a listening CM ID after
/// `rdma_bind_addr`/`rdma_listen` have filled in the effective
/// bound address.
///
/// # Safety
/// `cm_id` must be a valid, bound `rdma_cm_id` pointer.
pub(crate) unsafe fn extract_src_addr(cm_id: *mut ffi::rdma_cm_id) -> io::Result<SocketAddr> {
    // SAFETY: caller guarantees validity.  The field path is
    // `(*cm_id).route.addr.__bindgen_anon_1.src_addr`, where
    // `__bindgen_anon_1` is the source-address union containing
    // `sockaddr`/`sockaddr_in`/`sockaddr_in6`/`sockaddr_storage`.
    // bindgen's union name was verified against the generated
    // bindings on the target Linux build.
    let sa_ptr = unsafe { &(*cm_id).route.addr.__bindgen_anon_1.src_addr } as *const ffi::sockaddr;
    unsafe { sockaddr_to_socket_addr(sa_ptr.cast()) }
}

/// Extracts the destination address stored on a connected CM ID.
///
/// # Safety
/// `cm_id` must be a valid, connected `rdma_cm_id` pointer.
pub(crate) unsafe fn extract_dst_addr(cm_id: *mut ffi::rdma_cm_id) -> io::Result<SocketAddr> {
    // SAFETY: caller guarantees validity.  See `extract_src_addr`
    // for the union path rationale — `__bindgen_anon_2` is the
    // destination-address union.
    let sa_ptr = unsafe { &(*cm_id).route.addr.__bindgen_anon_2.dst_addr } as *const ffi::sockaddr;
    unsafe { sockaddr_to_socket_addr(sa_ptr.cast()) }
}

/// Cleanup helper: tears down the CM ID + QP + CQ + PD chain for
/// a partially-accepted connection when something fails along the
/// accept path.
pub(crate) unsafe fn cleanup_accept_resources(
    cm_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
    had_qp: bool,
) {
    unsafe {
        if had_qp && !cm_id.is_null() {
            ffi::rdma_destroy_qp(cm_id);
        }
        if !cq.is_null() {
            ffi::ibv_destroy_cq(cq);
        }
        if !pd.is_null() {
            ffi::ibv_dealloc_pd(pd);
        }
        if !cm_id.is_null() {
            ffi::rdma_destroy_id(cm_id);
        }
    }
}

/// Waits for a specific RDMA CM event on a non-blocking event
/// channel.  Returns the `cm_id` from the event on success.
///
/// Loops via [`AsyncFd::readable`] → `rdma_get_cm_event` → check
/// type → ack.  On `EAGAIN` clears readiness and re-arms.  Any
/// unexpected event type is acked and skipped (logged at WARN).
///
/// Used by both the `accept` path (waiting for
/// `CONNECT_REQUEST`) and the `connect` path (waiting for
/// `ADDR_RESOLVED`, `ROUTE_RESOLVED`, `ESTABLISHED`).  See #48.
pub(crate) async fn wait_for_cm_event(
    async_fd: &AsyncFd<BorrowedEventFd>,
    event_channel: *mut ffi::rdma_event_channel,
    expected: ffi::rdma_cm_event_type,
) -> io::Result<*mut ffi::rdma_cm_id> {
    tracing::debug!(expected, "wait_for_cm_event: entering loop");
    loop {
        tracing::debug!(expected, "wait_for_cm_event: awaiting readable");
        let mut guard = async_fd.readable().await?;
        tracing::debug!(
            expected,
            "wait_for_cm_event: fd is readable, polling cm event"
        );

        let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
        let ret = unsafe { ffi::rdma_get_cm_event(event_channel, &mut event) };
        if ret != 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN) || err.kind() == io::ErrorKind::WouldBlock {
                tracing::debug!(expected, "wait_for_cm_event: EAGAIN, clearing readiness");
                guard.clear_ready();
                continue;
            }
            return Err(err);
        }

        let event_type = unsafe { (*event).event };
        let event_status = unsafe { (*event).status };
        let id = unsafe { (*event).id };
        tracing::debug!(expected, event_type, event_status, "wait_for_cm_event: got event");
        unsafe { ffi::rdma_ack_cm_event(event) };

        if event_type == expected {
            tracing::debug!(expected, "wait_for_cm_event: matched, returning");
            return Ok(id);
        }

        // Terminal failure events must surface as errors immediately
        // rather than looping forever waiting for the expected event.
        let is_terminal = matches!(
            event_type,
            ffi::rdma_cm_event_type_RDMA_CM_EVENT_ADDR_ERROR
                | ffi::rdma_cm_event_type_RDMA_CM_EVENT_ROUTE_ERROR
                | ffi::rdma_cm_event_type_RDMA_CM_EVENT_CONNECT_ERROR
                | ffi::rdma_cm_event_type_RDMA_CM_EVENT_REJECTED
                | ffi::rdma_cm_event_type_RDMA_CM_EVENT_UNREACHABLE
                | ffi::rdma_cm_event_type_RDMA_CM_EVENT_DISCONNECTED
        );
        if is_terminal {
            return Err(io::Error::other(format!(
                "terminal CM event while waiting for {expected}: got {event_type} status {event_status}"
            )));
        }

        tracing::warn!(
            event_type,
            event_status,
            expected,
            "unexpected CM event (expected {expected}, got {event_type}), skipping"
        );
    }
}

/// Creates PD + CQ + QP on a CM ID, mirroring the resource-setup
/// block used in both the `accept` and `connect` paths.
///
/// On success returns `(pd, cq)`.  On failure tears down partial
/// resources and returns an error (the cm_id itself is NOT
/// destroyed — the caller owns that).
///
/// # Safety
/// `cm_id` must be a valid `rdma_cm_id` with a non-null `verbs`
/// context.
pub(crate) unsafe fn create_qp_resources(
    cm_id: *mut ffi::rdma_cm_id,
) -> io::Result<(*mut ffi::ibv_pd, *mut ffi::ibv_cq)> {
    let verbs = unsafe { (*cm_id).verbs };
    if verbs.is_null() {
        return Err(io::Error::other("CM ID has no verbs context"));
    }

    let pd = unsafe { ffi::ibv_alloc_pd(verbs) };
    if pd.is_null() {
        return Err(io::Error::other("ibv_alloc_pd failed"));
    }

    let cq_size = crate::connection::CQ_CAPACITY as i32;
    let cq = unsafe { ffi::ibv_create_cq(verbs, cq_size, ptr::null_mut(), ptr::null_mut(), 0) };
    if cq.is_null() {
        unsafe { ffi::ibv_dealloc_pd(pd) };
        return Err(io::Error::other("ibv_create_cq failed"));
    }

    let mut qp_attr: ffi::ibv_qp_init_attr = unsafe { std::mem::zeroed() };
    qp_attr.send_cq = cq;
    qp_attr.recv_cq = cq;
    qp_attr.qp_type = ffi::ibv_qp_type_IBV_QPT_RC;
    qp_attr.cap.max_send_wr = 16;
    qp_attr.cap.max_recv_wr = 16;
    qp_attr.cap.max_send_sge = 1;
    qp_attr.cap.max_recv_sge = 1;

    let ret = unsafe { ffi::rdma_create_qp(cm_id, pd, &mut qp_attr) };
    if ret != 0 {
        unsafe {
            ffi::ibv_destroy_cq(cq);
            ffi::ibv_dealloc_pd(pd);
        }
        return Err(io::Error::other(format!("rdma_create_qp failed: {ret}")));
    }

    Ok((pd, cq))
}

impl LocalListener for RdmaListener {
    type Connection = RdmaConnection;
    type Error = io::Error;

    async fn accept(&mut self) -> io::Result<RdmaConnection> {
        let async_fd = self
            .async_fd
            .as_ref()
            .ok_or_else(|| io::Error::other("RdmaListener was dropped or partially initialised"))?;

        // Wait for an incoming connection request via the shared
        // `wait_for_cm_event` helper (see #48).
        let new_id = wait_for_cm_event(
            async_fd,
            self.event_channel,
            ffi::rdma_cm_event_type_RDMA_CM_EVENT_CONNECT_REQUEST,
        )
        .await?;

        // Create PD + CQ + QP on the new CM ID.
        let (pd, cq) = match unsafe { create_qp_resources(new_id) } {
            Ok(res) => res,
            Err(e) => {
                unsafe { ffi::rdma_destroy_id(new_id) };
                return Err(e);
            }
        };

        // Accept the connection.
        let mut conn_param: ffi::rdma_conn_param = unsafe { std::mem::zeroed() };
        conn_param.initiator_depth = 1;
        conn_param.responder_resources = 1;
        let ret = unsafe { ffi::rdma_accept(new_id, &mut conn_param) };
        if ret != 0 {
            unsafe { cleanup_accept_resources(new_id, pd, cq, true) };
            return Err(io::Error::other(format!("rdma_accept failed: {ret}")));
        }

        // Extract the real peer address from the connected CM ID.
        let peer_addr = match unsafe { extract_dst_addr(new_id) } {
            Ok(real) => real,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not extract peer address, falling back to local_addr"
                );
                self.local_addr
            }
        };

        // Hand ownership of cm_id/pd/cq to the connection.
        match unsafe {
            RdmaConnection::from_accepted_cm_id(new_id, pd, cq, peer_addr, self.max_msg_size)
        } {
            Ok(conn) => {
                tracing::info!(%peer_addr, "RDMA connection accepted");
                Ok(conn)
            }
            Err(e) => {
                unsafe { cleanup_accept_resources(new_id, pd, cq, true) };
                Err(e)
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}

impl Drop for RdmaListener {
    fn drop(&mut self) {
        // Deregister from the tokio reactor BEFORE closing the
        // underlying fd via rdma_destroy_event_channel.  This
        // prevents a stale-fd race where another thread could
        // reclaim the same numeric fd in between.
        drop(self.async_fd.take());
        unsafe {
            if !self.listen_id.is_null() {
                ffi::rdma_destroy_id(self.listen_id);
            }
            if !self.event_channel.is_null() {
                ffi::rdma_destroy_event_channel(self.event_channel);
            }
        }
    }
}
