//! RDMA CM listener wrapping `rdma_cm_id` in listening mode.

use crate::connection::RdmaConnection;
use crate::ffi;
use ironsbe_transport::traits::LocalListener;
use std::io;
use std::net::SocketAddr;
use std::ptr;

/// Default backlog for `rdma_listen`.
const LISTEN_BACKLOG: i32 = 16;

/// Default max message size for accepted connections.
const DEFAULT_MAX_MSG_SIZE: usize = 64 * 1024;

/// RDMA CM listener.
///
/// Wraps a `rdma_cm_id` in listening mode.  `accept()` waits for an
/// incoming RDMA CM connection request, creates a QP on the new CM
/// ID, and returns an [`RdmaConnection`].
pub struct RdmaListener {
    listen_id: *mut ffi::rdma_cm_id,
    event_channel: *mut ffi::rdma_event_channel,
    local_addr: SocketAddr,
    max_msg_size: usize,
}

impl RdmaListener {
    /// Binds and listens on `addr`.
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
            return Err(io::Error::other(format!("rdma_create_id failed: {ret}")));
        }

        // Convert SocketAddr to sockaddr_in.
        let sockaddr = match addr {
            SocketAddr::V4(v4) => {
                let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
                sa.sin_family = libc::AF_INET as u16;
                sa.sin_port = v4.port().to_be();
                sa.sin_addr.s_addr = u32::from_ne_bytes(v4.ip().octets());
                sa
            }
            SocketAddr::V6(_) => {
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
            return Err(io::Error::other(format!("rdma_bind_addr failed: {ret}")));
        }

        let ret = unsafe { ffi::rdma_listen(listen_id, LISTEN_BACKLOG) };
        if ret != 0 {
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

impl LocalListener for RdmaListener {
    type Connection = RdmaConnection;
    type Error = io::Error;

    async fn accept(&mut self) -> io::Result<RdmaConnection> {
        loop {
            let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
            let ret = unsafe { ffi::rdma_get_cm_event(self.event_channel, &mut event) };
            if ret != 0 {
                tokio::task::yield_now().await;
                continue;
            }

            let event_type = unsafe { (*event).event };
            let new_id = unsafe { (*event).id };
            unsafe { ffi::rdma_ack_cm_event(event) };

            if event_type != ffi::rdma_cm_event_type_RDMA_CM_EVENT_CONNECT_REQUEST {
                continue;
            }

            // Create a QP on the new CM ID.
            let verbs = unsafe { (*new_id).verbs };
            if verbs.is_null() {
                tracing::warn!("accepted CM ID has no verbs context, skipping");
                unsafe { ffi::rdma_destroy_id(new_id) };
                continue;
            }

            let pd = unsafe { ffi::ibv_alloc_pd(verbs) };
            if pd.is_null() {
                tracing::warn!("ibv_alloc_pd failed for accepted connection");
                unsafe { ffi::rdma_destroy_id(new_id) };
                continue;
            }

            let cq_size = 32i32;
            let cq = unsafe {
                ffi::ibv_create_cq(verbs, cq_size, ptr::null_mut(), ptr::null_mut(), 0)
            };
            if cq.is_null() {
                tracing::warn!("ibv_create_cq failed for accepted connection");
                unsafe {
                    ffi::ibv_dealloc_pd(pd);
                    ffi::rdma_destroy_id(new_id);
                }
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
                tracing::warn!("rdma_create_qp failed: {ret}");
                unsafe {
                    ffi::ibv_destroy_cq(cq);
                    ffi::ibv_dealloc_pd(pd);
                    ffi::rdma_destroy_id(new_id);
                }
                continue;
            }

            // Accept the connection.
            let mut conn_param: ffi::rdma_conn_param = unsafe { std::mem::zeroed() };
            conn_param.initiator_depth = 1;
            conn_param.responder_resources = 1;
            let ret = unsafe { ffi::rdma_accept(new_id, &mut conn_param) };
            if ret != 0 {
                tracing::warn!("rdma_accept failed: {ret}");
                unsafe {
                    ffi::rdma_destroy_qp(new_id);
                    ffi::ibv_destroy_cq(cq);
                    ffi::ibv_dealloc_pd(pd);
                    ffi::rdma_destroy_id(new_id);
                }
                continue;
            }

            // Build the connection.  Note: RdmaConnection::from_cm_id
            // allocates its own PD/CQ/MRs, so we destroy the ones we
            // created above and let the connection own fresh ones.
            //
            // TODO: refactor to pass the already-created PD/CQ through
            // instead of double-allocating.
            unsafe {
                ffi::ibv_destroy_cq(cq);
                ffi::ibv_dealloc_pd(pd);
            }

            let peer_addr = self.local_addr; // approximation for now
            let conn = unsafe {
                RdmaConnection::from_cm_id(new_id, peer_addr, self.max_msg_size)
            }?;

            tracing::info!("RDMA connection accepted");
            return Ok(conn);
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
