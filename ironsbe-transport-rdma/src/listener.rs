//! RDMA CM listener wrapping `rdma_cm_id` in listening mode.

use crate::connection::RdmaConnection;
use crate::ffi;
use ironsbe_transport::traits::LocalListener;
use std::io;
use std::net::SocketAddr;
use std::ptr;

/// Default backlog for `rdma_listen`.
const LISTEN_BACKLOG: i32 = 16;

/// RDMA CM listener.
///
/// Wraps a `rdma_cm_id` in listening mode.  `accept()` waits for an
/// incoming RDMA CM connection request, creates a QP + PD + CQ on
/// the new CM ID, hands them to [`RdmaConnection`] for ownership,
/// and returns the new connection.
pub struct RdmaListener {
    listen_id: *mut ffi::rdma_cm_id,
    event_channel: *mut ffi::rdma_event_channel,
    local_addr: SocketAddr,
    max_msg_size: usize,
}

impl RdmaListener {
    /// Binds and listens on `addr`.
    ///
    /// On any error, releases partially-allocated resources before
    /// returning.
    ///
    /// # Errors
    /// Returns an `io::Error` if any RDMA CM call fails.
    pub fn bind(addr: SocketAddr, max_msg_size: usize) -> io::Result<Self> {
        let ec = unsafe { ffi::rdma_create_event_channel() };
        if ec.is_null() {
            return Err(io::Error::other("rdma_create_event_channel failed"));
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

        // Convert SocketAddr to sockaddr_in.  `sin_addr.s_addr` is
        // in network byte order, so build it from BE bytes.
        let sockaddr = match addr {
            SocketAddr::V4(v4) => {
                let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                sa.sin_family = libc::AF_INET as u16;
                sa.sin_port = v4.port().to_be();
                sa.sin_addr.s_addr = u32::from_be_bytes(v4.ip().octets());
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

        tracing::info!(%addr, "RDMA listener bound");

        Ok(Self {
            listen_id,
            event_channel: ec,
            local_addr: addr,
            max_msg_size,
        })
    }
}

/// Cleanup helper: tears down the CM ID + QP + CQ + PD chain for
/// a partially-accepted connection when something fails along the
/// accept path.
unsafe fn cleanup_accept_resources(
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

impl LocalListener for RdmaListener {
    type Connection = RdmaConnection;
    type Error = io::Error;

    async fn accept(&mut self) -> io::Result<RdmaConnection> {
        loop {
            let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
            let ret = unsafe { ffi::rdma_get_cm_event(self.event_channel, &mut event) };
            if ret != 0 {
                // Surface the real errno instead of spinning forever.
                return Err(io::Error::last_os_error());
            }

            let event_type = unsafe { (*event).event };
            let new_id = unsafe { (*event).id };
            unsafe { ffi::rdma_ack_cm_event(event) };

            if event_type != ffi::rdma_cm_event_type_RDMA_CM_EVENT_CONNECT_REQUEST {
                continue;
            }

            // Create PD + CQ + QP on the new CM ID.  From here on
            // any failure path must tear down the partial resources.
            let verbs = unsafe { (*new_id).verbs };
            if verbs.is_null() {
                unsafe { ffi::rdma_destroy_id(new_id) };
                tracing::warn!("accepted CM ID has no verbs context, skipping");
                continue;
            }

            let pd = unsafe { ffi::ibv_alloc_pd(verbs) };
            if pd.is_null() {
                unsafe { ffi::rdma_destroy_id(new_id) };
                tracing::warn!("ibv_alloc_pd failed for accepted connection");
                continue;
            }

            let cq_size = 32i32;
            let cq = unsafe {
                ffi::ibv_create_cq(verbs, cq_size, ptr::null_mut(), ptr::null_mut(), 0)
            };
            if cq.is_null() {
                unsafe { cleanup_accept_resources(new_id, pd, ptr::null_mut(), false) };
                tracing::warn!("ibv_create_cq failed for accepted connection");
                continue;
            }

            let mut qp_attr: ffi::ibv_qp_init_attr = unsafe { std::mem::zeroed() };
            qp_attr.send_cq = cq;
            qp_attr.recv_cq = cq;
            qp_attr.qp_type = ffi::ibv_qp_type_IBV_QPT_RC;
            qp_attr.cap.max_send_wr = 16;
            qp_attr.cap.max_recv_wr = 16;
            qp_attr.cap.max_send_sge = 1;
            qp_attr.cap.max_recv_sge = 1;

            let ret = unsafe { ffi::rdma_create_qp(new_id, pd, &mut qp_attr) };
            if ret != 0 {
                unsafe { cleanup_accept_resources(new_id, pd, cq, false) };
                tracing::warn!("rdma_create_qp failed: {ret}");
                continue;
            }

            // Accept the connection.
            let mut conn_param: ffi::rdma_conn_param = unsafe { std::mem::zeroed() };
            conn_param.initiator_depth = 1;
            conn_param.responder_resources = 1;
            let ret = unsafe { ffi::rdma_accept(new_id, &mut conn_param) };
            if ret != 0 {
                unsafe { cleanup_accept_resources(new_id, pd, cq, true) };
                tracing::warn!("rdma_accept failed: {ret}");
                continue;
            }

            // Hand ownership of cm_id/pd/cq to the connection.  On
            // success the connection's Drop owns cleanup; on failure
            // we release all resources explicitly.
            let peer_addr = self.local_addr; // TODO: extract real peer (tracked in follow-up)
            match unsafe {
                RdmaConnection::from_accepted_cm_id(
                    new_id,
                    pd,
                    cq,
                    peer_addr,
                    self.max_msg_size,
                )
            } {
                Ok(conn) => {
                    tracing::info!("RDMA connection accepted");
                    return Ok(conn);
                }
                Err(e) => {
                    unsafe { cleanup_accept_resources(new_id, pd, cq, true) };
                    return Err(e);
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}

impl Drop for RdmaListener {
    fn drop(&mut self) {
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
