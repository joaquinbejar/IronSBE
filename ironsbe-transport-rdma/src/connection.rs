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

use crate::ffi;
use bytes::BytesMut;
use ironsbe_transport::traits::LocalConnection;
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::ptr;
use std::rc::Rc;

/// Length prefix size (matches all other IronSBE backends).
const LENGTH_PREFIX_BYTES: usize = 4;

/// Number of pre-posted RECV work requests.
const RECV_DEPTH: usize = 16;

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
            _not_send: PhantomData,
        };

        // Pre-post all receive buffers.  If any post fails, the
        // connection's Drop will clean up everything.
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
        let ret = unsafe { ffi::ironsbe_ibv_post_recv(qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            return Err(io::Error::other(format!("ibv_post_recv failed: {ret}")));
        }
        Ok(())
    }
}

impl LocalConnection for RdmaConnection {
    type Error = io::Error;

    async fn recv(&mut self) -> io::Result<Option<BytesMut>> {
        loop {
            let mut wc: ffi::ibv_wc = unsafe { std::mem::zeroed() };
            let n = unsafe { ffi::ironsbe_ibv_poll_cq(self.cq, 1, &mut wc) };
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
            if wc.opcode == ffi::ibv_wc_opcode_IBV_WC_RECV {
                let idx = wc.wr_id as usize;
                let byte_len = wc.byte_len as usize;
                if byte_len < LENGTH_PREFIX_BYTES {
                    self.post_recv(idx)?;
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "malformed RDMA frame: byte_len {byte_len} < prefix {LENGTH_PREFIX_BYTES}"
                        ),
                    ));
                }
                let buf = &self.recv_bufs[idx];
                let msg_len =
                    u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
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
                        format!(
                            "truncated RDMA frame: declared {total} bytes, got {byte_len}"
                        ),
                    ));
                }
                let payload = BytesMut::from(&buf[LENGTH_PREFIX_BYTES..total]);
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
        let frame_len = u32::try_from(msg.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("message length {} exceeds u32::MAX", msg.len()),
            )
        })?;
        self.send_buf[..LENGTH_PREFIX_BYTES]
            .copy_from_slice(&frame_len.to_le_bytes());
        self.send_buf[LENGTH_PREFIX_BYTES..LENGTH_PREFIX_BYTES + msg.len()]
            .copy_from_slice(msg);
        let total = LENGTH_PREFIX_BYTES.checked_add(msg.len()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "framed message length overflow",
            )
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
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
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
        }
    }
}
