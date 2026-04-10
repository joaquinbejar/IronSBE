//! RDMA connection wrapping an ibverbs Queue Pair (QP).
//!
//! Each [`RdmaConnection`] owns a connected `rdma_cm_id` with a QP,
//! a Protection Domain, a Completion Queue, and pre-registered memory
//! regions for send and receive buffers.
//!
//! The SBE framing is the same as the other backends: each message is
//! a 4-byte little-endian length prefix followed by the payload.
//! Each SEND work request carries exactly one framed message; each
//! RECV work request expects exactly one.

use crate::ffi;
use bytes::BytesMut;
use ironsbe_transport::traits::LocalConnection;
use std::io;
use std::net::SocketAddr;
use std::ptr;

/// Length prefix size (matches all other IronSBE backends).
const LENGTH_PREFIX_BYTES: usize = 4;

/// Default max message size in bytes (excluding the 4-byte prefix).
const DEFAULT_MAX_MSG_SIZE: usize = 64 * 1024;

/// Number of pre-posted RECV work requests so the QP always has
/// somewhere to land incoming messages.
const RECV_DEPTH: usize = 16;

/// An established RDMA connection.
///
/// Not `Send` — all operations must happen on the thread that created
/// the connection (QP and CQ are per-thread resources in practice).
pub struct RdmaConnection {
    cm_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
    /// Pre-registered send buffer (length prefix + payload).
    send_buf: Vec<u8>,
    send_mr: *mut ffi::ibv_mr,
    /// Pre-registered receive buffers.
    recv_bufs: Vec<Vec<u8>>,
    recv_mrs: Vec<*mut ffi::ibv_mr>,
    /// Index into recv_bufs for the next RECV to re-post.
    recv_idx: usize,
    max_msg_size: usize,
    peer_addr: SocketAddr,
}

impl RdmaConnection {
    /// Creates a new connection from an already-established CM ID.
    ///
    /// The caller must have already called `rdma_create_qp` on the
    /// CM ID.  This function allocates PD, CQ, memory regions, and
    /// pre-posts RECV work requests.
    ///
    /// # Safety
    /// `cm_id` must be a valid, connected `rdma_cm_id` with a QP.
    pub(crate) unsafe fn from_cm_id(
        cm_id: *mut ffi::rdma_cm_id,
        peer_addr: SocketAddr,
        max_msg_size: usize,
    ) -> io::Result<Self> {
        let verbs = unsafe { (*cm_id).verbs };
        if verbs.is_null() {
            return Err(io::Error::other("rdma_cm_id has no verbs context"));
        }

        // Allocate PD.
        let pd = unsafe { ffi::ibv_alloc_pd(verbs) };
        if pd.is_null() {
            return Err(io::Error::other("ibv_alloc_pd failed"));
        }

        // Create CQ.
        let cq_size = (RECV_DEPTH + 1) as i32; // +1 for send
        let cq = unsafe { ffi::ibv_create_cq(verbs, cq_size, ptr::null_mut(), ptr::null_mut(), 0) };
        if cq.is_null() {
            return Err(io::Error::other("ibv_create_cq failed"));
        }

        let buf_size = LENGTH_PREFIX_BYTES + max_msg_size;

        // Register send buffer.
        let mut send_buf = vec![0u8; buf_size];
        let send_mr = unsafe {
            ffi::ibv_reg_mr(
                pd,
                send_buf.as_mut_ptr().cast(),
                buf_size,
                (ffi::IBV_ACCESS_LOCAL_WRITE | ffi::IBV_ACCESS_REMOTE_WRITE) as i32,
            )
        };
        if send_mr.is_null() {
            return Err(io::Error::other("ibv_reg_mr (send) failed"));
        }

        // Register receive buffers and pre-post RECVs.
        let mut recv_bufs = Vec::with_capacity(RECV_DEPTH);
        let mut recv_mrs = Vec::with_capacity(RECV_DEPTH);
        for _ in 0..RECV_DEPTH {
            let mut buf = vec![0u8; buf_size];
            let mr = unsafe {
                ffi::ibv_reg_mr(
                    pd,
                    buf.as_mut_ptr().cast(),
                    buf_size,
                    ffi::IBV_ACCESS_LOCAL_WRITE as i32,
                )
            };
            if mr.is_null() {
                return Err(io::Error::other("ibv_reg_mr (recv) failed"));
            }
            recv_bufs.push(buf);
            recv_mrs.push(mr);
        }

        let mut conn = Self {
            cm_id,
            pd,
            cq,
            send_buf,
            send_mr,
            recv_bufs,
            recv_mrs,
            recv_idx: 0,
            max_msg_size,
            peer_addr,
        };

        // Pre-post all receive buffers.
        for i in 0..RECV_DEPTH {
            conn.post_recv(i)?;
        }

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
        let ret = unsafe { ffi::ibv_post_recv(qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            return Err(io::Error::other(format!("ibv_post_recv failed: {ret}")));
        }
        Ok(())
    }
}

impl LocalConnection for RdmaConnection {
    type Error = io::Error;

    async fn recv(&mut self) -> io::Result<Option<BytesMut>> {
        // Poll the CQ for a RECV completion.
        loop {
            let mut wc: ffi::ibv_wc = unsafe { std::mem::zeroed() };
            let n = unsafe { ffi::ibv_poll_cq(self.cq, 1, &mut wc) };
            if n < 0 {
                return Err(io::Error::other("ibv_poll_cq failed"));
            }
            if n == 0 {
                tokio::task::yield_now().await;
                continue;
            }
            if wc.status != ffi::ibv_wc_status_IBV_WC_SUCCESS {
                return Err(io::Error::other(format!(
                    "RDMA work completion error: status={}",
                    wc.status
                )));
            }
            // Check if this is a RECV completion (not a SEND).
            if wc.opcode == ffi::ibv_wc_opcode_IBV_WC_RECV {
                let idx = wc.wr_id as usize;
                let byte_len = wc.byte_len as usize;
                if byte_len < LENGTH_PREFIX_BYTES {
                    // Re-post and skip malformed message.
                    self.post_recv(idx)?;
                    continue;
                }
                let buf = &self.recv_bufs[idx];
                let msg_len =
                    u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                let total = LENGTH_PREFIX_BYTES + msg_len;
                if total > byte_len || msg_len > self.max_msg_size {
                    self.post_recv(idx)?;
                    continue;
                }
                let payload =
                    BytesMut::from(&buf[LENGTH_PREFIX_BYTES..total]);
                // Re-post the buffer for the next message.
                self.post_recv(idx)?;
                return Ok(Some(payload));
            }
            // SEND completion — ignore and continue polling.
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
        let frame_len = msg.len() as u32;
        self.send_buf[..LENGTH_PREFIX_BYTES]
            .copy_from_slice(&frame_len.to_le_bytes());
        self.send_buf[LENGTH_PREFIX_BYTES..LENGTH_PREFIX_BYTES + msg.len()]
            .copy_from_slice(msg);
        let total = LENGTH_PREFIX_BYTES + msg.len();

        let mut sge = ffi::ibv_sge {
            addr: self.send_buf.as_ptr() as u64,
            length: total as u32,
            lkey: unsafe { (*self.send_mr).lkey },
        };
        let mut wr: ffi::ibv_send_wr = unsafe { std::mem::zeroed() };
        wr.sg_list = &mut sge;
        wr.num_sge = 1;
        wr.opcode = ffi::ibv_wr_opcode_IBV_WR_SEND;
        wr.send_flags = ffi::ibv_send_flags_IBV_SEND_SIGNALED;

        let mut bad_wr: *mut ffi::ibv_send_wr = ptr::null_mut();
        let qp = unsafe { (*self.cm_id).qp };
        let ret = unsafe { ffi::ibv_post_send(qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            return Err(io::Error::other(format!("ibv_post_send failed: {ret}")));
        }
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }
}

impl Drop for RdmaConnection {
    fn drop(&mut self) {
        unsafe {
            // Deregister MRs.
            if !self.send_mr.is_null() {
                ffi::ibv_dereg_mr(self.send_mr);
            }
            for mr in &self.recv_mrs {
                if !mr.is_null() {
                    ffi::ibv_dereg_mr(*mr);
                }
            }
            // Destroy CQ.
            if !self.cq.is_null() {
                ffi::ibv_destroy_cq(self.cq);
            }
            // Dealloc PD.
            if !self.pd.is_null() {
                ffi::ibv_dealloc_pd(self.pd);
            }
            // Disconnect + destroy CM ID.
            if !self.cm_id.is_null() {
                ffi::rdma_disconnect(self.cm_id);
                ffi::rdma_destroy_id(self.cm_id);
            }
        }
    }
}
