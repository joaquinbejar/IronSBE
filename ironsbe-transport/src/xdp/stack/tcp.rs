//! `smoltcp`-based TCP stack on top of the AF_XDP datapath.
//!
//! Wire-compatible with `tcp-tokio` / `tcp-uring`: peers can be unmodified
//! standard TCP clients.  Higher complexity than [`super::udp::UdpStack`]
//! but recovers full TCP semantics (reordering, retransmission, flow
//! control) inside userspace.
//!
//! See `docs/transport-backends.md` for the operational details.

#![allow(clippy::module_name_repetitions)]

use crate::traits::LocalConnection;
use crate::xdp::frames::MacAddr;
use crate::xdp::stack::{FrameTxQueue, XdpStack};
use bytes::{Bytes, BytesMut};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::SystemTime;

/// Default TCP socket buffer size used when the caller does not override
/// it via [`SmoltcpStackConfig`].
const DEFAULT_SOCKET_BUFFER_SIZE: usize = 64 * 1024;

/// Default maximum SBE frame size accepted by the length-prefix decoder.
const DEFAULT_MAX_FRAME_SIZE: usize = 64 * 1024;

/// Configuration for [`SmoltcpStack`].
#[derive(Debug, Clone)]
pub struct SmoltcpStackConfig {
    /// IPv4 address the stack will respond on.
    pub local_ip: Ipv4Addr,
    /// IPv4 prefix length (CIDR).
    pub local_prefix: u8,
    /// TCP port the stack will accept connections on.
    pub local_port: u16,
    /// Local hardware (MAC) address used as the L2 source for outbound
    /// frames.
    pub local_mac: MacAddr,
    /// Per-socket TCP receive buffer size in bytes.
    pub socket_buffer_size: usize,
    /// Maximum number of concurrent accepted TCP sessions.
    pub max_sessions: usize,
    /// Maximum SBE frame size in bytes.  Length-prefixed frames larger
    /// than this are rejected and the offending session is closed
    /// (otherwise a peer could advertise an arbitrarily large prefix and
    /// drive the per-session decode buffer to OOM).
    pub max_frame_size: usize,
}

impl SmoltcpStackConfig {
    /// Creates a new smoltcp stack config bound to a single IPv4 address
    /// and TCP port.
    #[must_use]
    pub fn new(local_ip: Ipv4Addr, local_prefix: u8, local_port: u16, local_mac: MacAddr) -> Self {
        Self {
            local_ip,
            local_prefix,
            local_port,
            local_mac,
            socket_buffer_size: DEFAULT_SOCKET_BUFFER_SIZE,
            max_sessions: 64,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }

    /// Sets the per-socket TCP buffer size.
    #[must_use]
    pub fn socket_buffer_size(mut self, size: usize) -> Self {
        self.socket_buffer_size = size;
        self
    }

    /// Sets the maximum number of concurrent accepted sessions.
    #[must_use]
    pub fn max_sessions(mut self, max: usize) -> Self {
        self.max_sessions = max;
        self
    }

    /// Sets the maximum SBE frame size accepted by the length-prefix
    /// decoder.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size;
        self
    }
}

/// `smoltcp::phy::Device` adapter that ferries frames through caller-owned
/// rx / tx queues instead of touching a kernel socket.  This is what makes
/// the smoltcp stack drivable from inside the AF_XDP poll loop.
///
/// `rx_queue` is filled by [`XdpStack::on_rx`] before the stack drives
/// `Interface::poll`; `tx_queue` is drained by [`XdpStack::on_rx`] /
/// [`XdpStack::poll_timers`] after each `Interface::poll`.
struct XdpDevice {
    rx_queue: Rc<RefCell<VecDeque<Vec<u8>>>>,
    tx_queue: Rc<RefCell<VecDeque<Vec<u8>>>>,
    mtu: usize,
}

impl XdpDevice {
    fn new(mtu: usize) -> Self {
        Self {
            rx_queue: Rc::new(RefCell::new(VecDeque::new())),
            tx_queue: Rc::new(RefCell::new(VecDeque::new())),
            mtu,
        }
    }
}

impl Device for XdpDevice {
    type RxToken<'a> = XdpRxToken;
    type TxToken<'a> = XdpTxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = self.mtu;
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let frame = self.rx_queue.borrow_mut().pop_front()?;
        Some((
            XdpRxToken { buffer: frame },
            XdpTxToken {
                tx_queue: Rc::clone(&self.tx_queue),
            },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(XdpTxToken {
            tx_queue: Rc::clone(&self.tx_queue),
        })
    }
}

/// RX token consuming a single inbound frame from the rx queue.
struct XdpRxToken {
    buffer: Vec<u8>,
}

impl RxToken for XdpRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer)
    }
}

/// TX token that pushes a built frame into the tx queue.
struct XdpTxToken {
    tx_queue: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl TxToken for XdpTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        self.tx_queue.borrow_mut().push_back(buf);
        r
    }
}

/// Per-session state stored inside [`SmoltcpStack`].
struct SessionState {
    handle: SocketHandle,
    /// Inbound SBE frames waiting to be consumed by `recv`.
    rx_queue: VecDeque<BytesMut>,
    /// Outbound SBE frames waiting to be pushed into the smoltcp socket on
    /// the next `Interface::poll`.
    tx_queue: VecDeque<Bytes>,
    /// Waker stored when `recv` is awaiting a frame.  Woken from the
    /// stack's `poll` loop the next time a frame is decoded into
    /// `rx_queue` or the session is closed.
    rx_waker: Option<Waker>,
    /// `true` once the session has been closed (peer FIN, oversized
    /// frame, application drop).  Causes pending `recv` futures to
    /// resolve to `Ok(None)`.
    closed: bool,
}

/// Length-prefix size in bytes (matches `SbeFrameCodec`).
const LENGTH_PREFIX_BYTES: usize = 4;

/// Inner state shared between [`SmoltcpStack`] and the [`SmoltcpConnection`]
/// handles it produces.
struct SmoltcpStackInner {
    config: SmoltcpStackConfig,
    iface: Interface,
    device: XdpDevice,
    sockets: SocketSet<'static>,
    /// Listening sockets that have not yet accepted a peer.  We keep a
    /// pool so we can re-arm a fresh listener after each accept.
    listening: Vec<SocketHandle>,
    /// Established sessions, keyed by smoltcp `SocketHandle`.
    sessions: Vec<SessionState>,
    pending_accepts: VecDeque<usize>,
    /// Per-session length-prefix decoder buffers.
    decode_buffers: Vec<BytesMut>,
}

impl SmoltcpStackInner {
    fn now() -> Instant {
        let dur = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        Instant::from_micros(dur.as_micros() as i64)
    }

    fn add_listener(&mut self) -> Result<SocketHandle, SmoltcpStackError> {
        let rx_buf = tcp::SocketBuffer::new(vec![0u8; self.config.socket_buffer_size]);
        let tx_buf = tcp::SocketBuffer::new(vec![0u8; self.config.socket_buffer_size]);
        let mut sock = tcp::Socket::new(rx_buf, tx_buf);
        sock.listen(self.config.local_port)
            .map_err(|e| SmoltcpStackError::Listen(e.to_string()))?;
        Ok(self.sockets.add(sock))
    }

    /// Drives one round of the smoltcp interface and returns the index
    /// of any session that should be torn down by the caller.
    fn poll(&mut self) -> Result<(), SmoltcpStackError> {
        // smoltcp 0.12's `Interface::poll` returns a `PollResult` (enum),
        // not a `Result`, so there is nothing to propagate here — but we
        // keep the function fallible so the OOM-on-oversized-frame path
        // and the listen-error path can both surface through one type.
        let _ = self
            .iface
            .poll(Self::now(), &mut self.device, &mut self.sockets);

        // Promote any newly-active listening sockets into accepted sessions.
        let mut to_promote = Vec::new();
        for (idx, handle) in self.listening.iter().copied().enumerate() {
            let sock = self.sockets.get::<tcp::Socket>(handle);
            if sock.is_active() {
                to_promote.push((idx, handle));
            }
        }
        // Reverse so removals don't shift indices.
        for (idx, handle) in to_promote.into_iter().rev() {
            self.listening.remove(idx);
            let session_idx = self.sessions.len();
            self.sessions.push(SessionState {
                handle,
                rx_queue: VecDeque::new(),
                tx_queue: VecDeque::new(),
                rx_waker: None,
                closed: false,
            });
            self.decode_buffers.push(BytesMut::new());
            self.pending_accepts.push_back(session_idx);
            // Re-arm a fresh listener for the next peer.
            if self.sessions.len() + self.listening.len() < self.config.max_sessions {
                let new_handle = self.add_listener()?;
                self.listening.push(new_handle);
            }
        }

        let max_frame_size = self.config.max_frame_size;

        // Drain bytes out of every active session, decode length-prefix
        // frames, and push completed frames into rx_queue.
        for (idx, session) in self.sessions.iter_mut().enumerate() {
            let sock = self.sockets.get_mut::<tcp::Socket>(session.handle);
            // Detect peer-side close so pending recv() futures resolve.
            if !sock.is_active() && !session.closed {
                session.closed = true;
                if let Some(waker) = session.rx_waker.take() {
                    waker.wake();
                }
            }
            let buf = &mut self.decode_buffers[idx];
            if sock.can_recv() {
                let _ = sock.recv(|slice| {
                    buf.extend_from_slice(slice);
                    (slice.len(), ())
                });
            }
            // Try to extract complete frames.  Reject any prefix that
            // exceeds `max_frame_size` so a malicious peer cannot drive
            // the per-session decode buffer to OOM.
            let mut emitted = false;
            loop {
                if buf.len() < LENGTH_PREFIX_BYTES {
                    break;
                }
                let frame_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                if frame_len > max_frame_size {
                    // Close the session and surface a clear error to the
                    // caller via the rx_queue → recv path.  We mark
                    // closed and clear the buffer; the dead session
                    // remains in `sessions` until the application drops
                    // its `SmoltcpConnection`.
                    session.closed = true;
                    buf.clear();
                    if let Some(waker) = session.rx_waker.take() {
                        waker.wake();
                    }
                    break;
                }
                let total = LENGTH_PREFIX_BYTES + frame_len;
                if buf.len() < total {
                    break;
                }
                let _ = buf.split_to(LENGTH_PREFIX_BYTES);
                let frame = buf.split_to(frame_len);
                session.rx_queue.push_back(frame);
                emitted = true;
            }
            if emitted && let Some(waker) = session.rx_waker.take() {
                waker.wake();
            }
            // Push outbound frames if the socket can send.  We dequeue
            // first so an oversized frame (which we reject) doesn't
            // permanently stall the session.
            while let Some(msg) = session.tx_queue.front().cloned() {
                if !sock.can_send() {
                    break;
                }
                let frame_len_u32 = match u32::try_from(msg.len()) {
                    Ok(v) => v,
                    Err(_) => {
                        // Drop the offending message and continue; the
                        // application caller already saw the error path
                        // because `send_owned` rejects > u32::MAX before
                        // this point.  This branch is defence-in-depth.
                        session.tx_queue.pop_front();
                        continue;
                    }
                };
                let prefix = frame_len_u32.to_le_bytes();
                let to_send: Vec<u8> = prefix.iter().copied().chain(msg.iter().copied()).collect();
                if sock.send_slice(&to_send).is_ok() {
                    session.tx_queue.pop_front();
                } else {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// `smoltcp`-based TCP stack.  Single-threaded; not `Send`.
///
/// `Clone` produces a shared handle (via `Rc` clone) to the same
/// internal state, mirroring [`super::udp::UdpStack`].  This is only
/// used to satisfy the `Clone` bound on `LocalTransport::BindConfig`.
#[derive(Clone)]
pub struct SmoltcpStack {
    inner: Rc<RefCell<SmoltcpStackInner>>,
}

impl SmoltcpStack {
    /// Creates a new `smoltcp` stack from the given config.
    ///
    /// # Errors
    /// Returns [`SmoltcpStackError::Listen`] if smoltcp rejects the
    /// listen call on the bound port (e.g. invalid `local_port = 0`).
    pub fn new(config: SmoltcpStackConfig) -> Result<Self, SmoltcpStackError> {
        let mut device = XdpDevice::new(1500);
        let hw = HardwareAddress::Ethernet(EthernetAddress(config.local_mac));
        let mut iface = Interface::new(Config::new(hw), &mut device, SmoltcpStackInner::now());
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(
                IpAddress::Ipv4(Ipv4Address::from(config.local_ip.octets())),
                config.local_prefix,
            ));
        });

        let mut inner = SmoltcpStackInner {
            config: config.clone(),
            iface,
            device,
            sockets: SocketSet::new(Vec::new()),
            listening: Vec::new(),
            sessions: Vec::new(),
            pending_accepts: VecDeque::new(),
            decode_buffers: Vec::new(),
        };
        // Pre-arm one listening socket; further listeners are added as
        // sessions get promoted.
        let h = inner.add_listener()?;
        inner.listening.push(h);

        Ok(Self {
            inner: Rc::new(RefCell::new(inner)),
        })
    }
}

/// Errors produced by the smoltcp stack.
#[derive(Debug, thiserror::Error)]
pub enum SmoltcpStackError {
    /// `tcp::Socket::listen` rejected the bound port (e.g. `local_port = 0`).
    #[error("smoltcp listen failed: {0}")]
    Listen(String),
}

impl XdpStack for SmoltcpStack {
    type Connection = SmoltcpConnection;
    type Error = SmoltcpStackError;

    fn on_rx(
        &mut self,
        frame: &[u8],
        out: &mut FrameTxQueue<'_>,
    ) -> Result<Option<SmoltcpConnection>, SmoltcpStackError> {
        // Push the inbound frame into the device rx queue.  We still copy
        // here because the smoltcp `Device::receive` API takes ownership of
        // the buffer; making it borrow `&[u8]` for the duration of `poll`
        // requires a Device adapter restructure that is tracked alongside
        // the actual `xsk-rs` datapath integration in the follow-up issue.
        {
            let inner = self.inner.borrow();
            inner.device.rx_queue.borrow_mut().push_back(frame.to_vec());
        }

        // Drive the smoltcp interface.
        self.inner.borrow_mut().poll()?;

        // Drain any frames smoltcp wants to send and pop the first
        // newly-accepted session in one borrow.  We deliberately do
        // **not** also drain `pending_accepts` outside of `on_rx` — the
        // trait return value is the canonical accept path.
        let mut new_conn_idx = None;
        {
            let mut inner = self.inner.borrow_mut();
            let tx_handle = Rc::clone(&inner.device.tx_queue);
            let mut tx = tx_handle.borrow_mut();
            while let Some(frame) = tx.pop_front() {
                out.push(frame);
            }
            new_conn_idx = inner.pending_accepts.pop_front().or(new_conn_idx);
        }
        Ok(new_conn_idx.map(|session_idx| SmoltcpConnection {
            session_idx,
            inner: Rc::clone(&self.inner),
        }))
    }

    fn poll_timers(&mut self, out: &mut FrameTxQueue<'_>) -> Result<(), SmoltcpStackError> {
        self.inner.borrow_mut().poll()?;
        let inner = self.inner.borrow();
        let mut tx = inner.device.tx_queue.borrow_mut();
        while let Some(frame) = tx.pop_front() {
            out.push(frame);
        }
        Ok(())
    }

    fn local_ip(&self) -> IpAddr {
        IpAddr::V4(self.inner.borrow().config.local_ip)
    }
}

/// One TCP session produced by [`SmoltcpStack`].
pub struct SmoltcpConnection {
    session_idx: usize,
    inner: Rc<RefCell<SmoltcpStackInner>>,
}

impl Drop for SmoltcpConnection {
    fn drop(&mut self) {
        // Mark the session closed so any in-flight `recv` future
        // resolves to `Ok(None)` rather than waiting forever.
        if let Ok(mut inner) = self.inner.try_borrow_mut()
            && let Some(session) = inner.sessions.get_mut(self.session_idx)
        {
            session.closed = true;
            if let Some(waker) = session.rx_waker.take() {
                waker.wake();
            }
        }
    }
}

/// Future returned by [`SmoltcpConnection::recv`].
struct SmoltcpRecvFuture {
    session_idx: usize,
    inner: Rc<RefCell<SmoltcpStackInner>>,
}

impl Future for SmoltcpRecvFuture {
    type Output = io::Result<Option<BytesMut>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.inner.borrow_mut();
        let session = match inner.sessions.get_mut(self.session_idx) {
            Some(s) => s,
            None => {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "session has been dropped",
                )));
            }
        };
        if let Some(frame) = session.rx_queue.pop_front() {
            return Poll::Ready(Ok(Some(frame)));
        }
        if session.closed {
            return Poll::Ready(Ok(None));
        }
        session.rx_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl LocalConnection for SmoltcpConnection {
    type Error = io::Error;

    fn recv(&mut self) -> impl Future<Output = io::Result<Option<BytesMut>>> + '_ {
        SmoltcpRecvFuture {
            session_idx: self.session_idx,
            inner: Rc::clone(&self.inner),
        }
    }

    async fn send(&mut self, msg: &[u8]) -> io::Result<()> {
        let owned = Bytes::copy_from_slice(msg);
        self.send_owned(owned).await
    }

    async fn send_owned(&mut self, msg: Bytes) -> io::Result<()> {
        // Validate length up-front so the caller sees the error before
        // the next poll round.
        if u32::try_from(msg.len()).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame length {} exceeds u32::MAX", msg.len()),
            ));
        }
        let mut inner = self.inner.borrow_mut();
        let session = inner.sessions.get_mut(self.session_idx).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "session has been dropped")
        })?;
        session.tx_queue.push_back(msg);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        // smoltcp tracks the remote endpoint inside the socket; reach in.
        let inner = self.inner.borrow();
        let session = inner.sessions.get(self.session_idx).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "session has been dropped")
        })?;
        let sock = inner.sockets.get::<tcp::Socket>(session.handle);
        let endpoint = sock
            .remote_endpoint()
            .ok_or_else(|| io::Error::other("smoltcp socket has no remote endpoint"))?;
        let ip = match endpoint.addr {
            IpAddress::Ipv4(v4) => IpAddr::V4(Ipv4Addr::from(v4.octets())),
            #[allow(unreachable_patterns)]
            _ => return Err(io::Error::other("non-ipv4 endpoint")),
        };
        Ok(SocketAddr::new(ip, endpoint.port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_MAC: MacAddr = [0x02, 0, 0, 0, 0, 0xaa];
    const LOCAL_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);

    #[test]
    fn test_stack_creation_succeeds() {
        let cfg = SmoltcpStackConfig::new(LOCAL_IP, 24, 9000, LOCAL_MAC)
            .socket_buffer_size(8 * 1024)
            .max_sessions(8)
            .max_frame_size(32 * 1024);
        let stack = SmoltcpStack::new(cfg);
        assert!(stack.is_ok(), "valid config must construct cleanly");
    }

    #[test]
    fn test_local_ip_returns_configured_address() {
        let cfg = SmoltcpStackConfig::new(LOCAL_IP, 24, 9000, LOCAL_MAC);
        let stack = SmoltcpStack::new(cfg).expect("valid config");
        assert_eq!(stack.local_ip(), IpAddr::V4(LOCAL_IP));
    }

    #[test]
    fn test_listen_on_port_zero_returns_error() {
        let cfg = SmoltcpStackConfig::new(LOCAL_IP, 24, 0, LOCAL_MAC);
        let result = SmoltcpStack::new(cfg);
        assert!(
            matches!(result, Err(SmoltcpStackError::Listen(_))),
            "listen on port 0 must fail loudly"
        );
    }

    #[test]
    fn test_max_frame_size_default() {
        let cfg = SmoltcpStackConfig::new(LOCAL_IP, 24, 9000, LOCAL_MAC);
        assert_eq!(cfg.max_frame_size, DEFAULT_MAX_FRAME_SIZE);
    }
}
