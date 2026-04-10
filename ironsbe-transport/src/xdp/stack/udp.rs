//! UDP-based [`XdpStack`](super::XdpStack) implementation.
//!
//! Each peer `(ip, port)` becomes one [`UdpConnection`].  Wire format on
//! the UDP datagram payload is the standard SBE 4-byte little-endian
//! length prefix followed by the message body.  Datagrams are not
//! reassembled across UDP boundaries: each SBE message must fit in one
//! datagram (subject to the path MTU).

#![allow(clippy::module_name_repetitions)]

use crate::traits::LocalConnection;
use crate::xdp::frames::{
    FrameError, MacAddr, build_arp_reply, build_udp_ipv4, parse_arp, parse_ipv4_udp,
};
use crate::xdp::stack::{FrameTxQueue, XdpStack};
use bytes::{Bytes, BytesMut};
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

/// Length-prefix size in bytes (matches `SbeFrameCodec`).
const LENGTH_PREFIX_BYTES: usize = 4;

/// Largest UDP/IPv4 payload that can be addressed by the 16-bit length
/// field, minus the UDP header (8 bytes).  This caps the on-wire payload
/// size, **including** our 4-byte SBE length prefix.
const MAX_UDP_PAYLOAD: usize = u16::MAX as usize - 8;

/// Default `max_frame_size` chosen so the on-wire UDP payload (prefix +
/// SBE message) fits inside a single IPv4 datagram with comfortable
/// headroom for the prefix.
const DEFAULT_MAX_FRAME_SIZE: usize = MAX_UDP_PAYLOAD - LENGTH_PREFIX_BYTES;

/// Per-peer connection state held inside [`UdpStack`].
struct ConnectionState {
    peer_ip: Ipv4Addr,
    peer_port: u16,
    peer_mac: MacAddr,
    /// Inbound SBE frames waiting to be consumed by `recv`.
    rx_queue: VecDeque<BytesMut>,
    /// Waker stored when `recv` is awaiting a frame.  Woken from `on_rx`
    /// the next time a frame for this peer is enqueued.
    rx_waker: Option<Waker>,
    /// `true` once the application drops the connection.  Causes pending
    /// `recv` futures to resolve to `Ok(None)` ("peer closed").
    closed: bool,
}

/// Configuration for [`UdpStack`].
#[derive(Debug, Clone)]
pub struct UdpStackConfig {
    /// IPv4 address the stack will respond on.
    pub local_ip: Ipv4Addr,
    /// UDP port the stack will accept SBE messages on.
    pub local_port: u16,
    /// Local hardware (MAC) address used as the L2 source for outbound
    /// frames and as the answer to ARP probes for `local_ip`.
    pub local_mac: MacAddr,
    /// Maximum SBE frame size in bytes.  Frames larger than this are
    /// rejected on both rx and tx.
    pub max_frame_size: usize,
}

impl UdpStackConfig {
    /// Creates a new UDP stack config with the given local L3/L2 identity.
    ///
    /// The default `max_frame_size` is the largest SBE message that can
    /// fit in a single IPv4/UDP datagram once the 4-byte length prefix
    /// is accounted for.
    #[must_use]
    pub fn new(local_ip: Ipv4Addr, local_port: u16, local_mac: MacAddr) -> Self {
        Self {
            local_ip,
            local_port,
            local_mac,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }

    /// Sets the maximum SBE frame size in bytes.
    ///
    /// The on-wire UDP payload is `size + 4` (length prefix); requests
    /// that would exceed `MAX_UDP_PAYLOAD` are silently clamped down so
    /// callers cannot accidentally configure an unrepresentable frame
    /// size.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size.min(MAX_UDP_PAYLOAD - LENGTH_PREFIX_BYTES);
        self
    }
}

/// Shared state between the [`UdpStack`] and the [`UdpConnection`] handles
/// it produces.
struct UdpStackInner {
    config: UdpStackConfig,
    /// Map from peer `(ip, port)` to the per-connection state.
    connections: HashMap<SocketAddrV4, ConnectionState>,
    /// Outbound frames built by `UdpConnection::send`.  The datapath
    /// drains this on its next poll round.
    pending_tx: VecDeque<Vec<u8>>,
}

impl UdpStackInner {
    fn build_outbound(&self, peer: SocketAddrV4, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
        let state = self
            .connections
            .get(&peer)
            .ok_or_else(|| FrameError::Malformed(format!("no connection state for peer {peer}")))?;
        // Length-prefixed framing on top of UDP.  We check the **on-wire**
        // size (prefix + payload), not just the payload, to make sure the
        // resulting datagram still fits inside the configured ceiling.
        let frame_len = payload.len();
        let datagram_len = LENGTH_PREFIX_BYTES.checked_add(frame_len).ok_or_else(|| {
            FrameError::Malformed(format!(
                "frame length overflow: prefix {LENGTH_PREFIX_BYTES} + payload {frame_len}"
            ))
        })?;
        let max_on_wire = self
            .config
            .max_frame_size
            .checked_add(LENGTH_PREFIX_BYTES)
            .unwrap_or(MAX_UDP_PAYLOAD);
        if datagram_len > max_on_wire {
            return Err(FrameError::Malformed(format!(
                "frame too large: on-wire UDP payload {datagram_len} exceeds limit {max_on_wire}"
            )));
        }
        let frame_len_u32 = u32::try_from(frame_len)
            .map_err(|_| FrameError::Malformed(format!("frame length {frame_len} > u32::MAX")))?;
        let mut datagram = Vec::with_capacity(datagram_len);
        datagram.extend_from_slice(&frame_len_u32.to_le_bytes());
        datagram.extend_from_slice(payload);
        build_udp_ipv4(
            self.config.local_mac,
            state.peer_mac,
            self.config.local_ip,
            state.peer_ip,
            self.config.local_port,
            state.peer_port,
            &datagram,
        )
    }
}

/// UDP-based stack.  Single-threaded; not `Send`.
///
/// `Clone` produces a shared handle (via `Rc` clone) to the same
/// internal state.  This is only used to satisfy the `Clone` bound on
/// `LocalTransport::BindConfig` at the type level; the stack is never
/// actually cloned at runtime.
#[derive(Clone)]
pub struct UdpStack {
    inner: Rc<RefCell<UdpStackInner>>,
}

impl UdpStack {
    /// Creates a new UDP stack from `config`.
    #[must_use]
    pub fn new(config: UdpStackConfig) -> Self {
        Self {
            inner: Rc::new(RefCell::new(UdpStackInner {
                config,
                connections: HashMap::new(),
                pending_tx: VecDeque::new(),
            })),
        }
    }

    /// Drains the outbound queue into `out`.  Called by the datapath after
    /// every rx round and timer poll.
    pub fn drain_pending_tx(&self, out: &mut FrameTxQueue<'_>) {
        let mut inner = self.inner.borrow_mut();
        while let Some(frame) = inner.pending_tx.pop_front() {
            out.push(frame);
        }
    }
}

/// Errors produced by the UDP stack.
#[derive(Debug, thiserror::Error)]
pub enum UdpStackError {
    /// Frame parsing or construction failed.
    #[error("frame error: {0}")]
    Frame(#[from] FrameError),
}

impl XdpStack for UdpStack {
    type Connection = UdpConnection;
    type Error = UdpStackError;

    fn on_rx(
        &mut self,
        frame: &[u8],
        out: &mut FrameTxQueue<'_>,
    ) -> Result<Option<UdpConnection>, UdpStackError> {
        // Try ARP first (cheap, common during startup).
        if let Some(arp) = parse_arp(frame).ok().flatten()
            && arp.operation == 1
            && arp.target_ip == self.inner.borrow().config.local_ip
        {
            let inner = self.inner.borrow();
            let reply = build_arp_reply(
                inner.config.local_mac,
                inner.config.local_ip,
                arp.sender_mac,
                arp.sender_ip,
            )?;
            out.push(reply);
            return Ok(None);
        }

        // Then UDP/IPv4.
        let Some(udp) = parse_ipv4_udp(frame).ok().flatten() else {
            return Ok(None);
        };
        if udp.dst_ip != self.inner.borrow().config.local_ip
            || udp.dst_port != self.inner.borrow().config.local_port
        {
            return Ok(None);
        }
        // Length-prefix decode of the UDP payload.
        if udp.payload.len() < LENGTH_PREFIX_BYTES {
            return Ok(None);
        }
        let frame_len = u32::from_le_bytes([
            udp.payload[0],
            udp.payload[1],
            udp.payload[2],
            udp.payload[3],
        ]) as usize;
        if frame_len > self.inner.borrow().config.max_frame_size {
            return Ok(None);
        }
        let total = LENGTH_PREFIX_BYTES
            .checked_add(frame_len)
            .filter(|t| *t <= udp.payload.len())
            .ok_or_else(|| {
                UdpStackError::Frame(FrameError::Malformed(format!(
                    "udp payload {} too short for frame_len {}",
                    udp.payload.len(),
                    frame_len
                )))
            })?;
        let payload = BytesMut::from(&udp.payload[LENGTH_PREFIX_BYTES..total]);

        // Look up the source MAC from the Ethernet header so we can talk
        // back to the same peer.
        let parsed_eth = crate::xdp::frames::parse_ethernet(frame)?;
        let peer_mac = parsed_eth.src_mac;
        let peer_addr = SocketAddrV4::new(udp.src_ip, udp.src_port);

        let mut inner = self.inner.borrow_mut();
        let new_conn = if let std::collections::hash_map::Entry::Vacant(slot) =
            inner.connections.entry(peer_addr)
        {
            slot.insert(ConnectionState {
                peer_ip: udp.src_ip,
                peer_port: udp.src_port,
                peer_mac,
                rx_queue: VecDeque::new(),
                rx_waker: None,
                closed: false,
            });
            true
        } else {
            false
        };
        if let Some(state) = inner.connections.get_mut(&peer_addr) {
            state.rx_queue.push_back(payload);
            // Wake any pending recv() future for this peer.
            if let Some(waker) = state.rx_waker.take() {
                waker.wake();
            }
        }
        drop(inner);

        if new_conn {
            Ok(Some(UdpConnection {
                peer: peer_addr,
                inner: Rc::clone(&self.inner),
            }))
        } else {
            Ok(None)
        }
    }

    fn poll_timers(&mut self, _out: &mut FrameTxQueue<'_>) -> Result<(), UdpStackError> {
        // UDP stack has no timers.
        Ok(())
    }

    fn local_ip(&self) -> IpAddr {
        IpAddr::V4(self.inner.borrow().config.local_ip)
    }
}

/// One peer connection produced by [`UdpStack`].
pub struct UdpConnection {
    peer: SocketAddrV4,
    inner: Rc<RefCell<UdpStackInner>>,
}

impl Drop for UdpConnection {
    fn drop(&mut self) {
        // Mark the connection closed so any in-flight `recv` future for
        // this peer resolves to `Ok(None)` rather than waiting forever.
        if let Ok(mut inner) = self.inner.try_borrow_mut()
            && let Some(state) = inner.connections.get_mut(&self.peer)
        {
            state.closed = true;
            if let Some(waker) = state.rx_waker.take() {
                waker.wake();
            }
        }
    }
}

/// Future returned by [`UdpConnection::recv`].
///
/// Resolves to `Ok(Some(frame))` when a frame is available for the peer,
/// `Ok(None)` when the connection has been closed (the application
/// dropped its [`UdpConnection`] handle), or `Err` if the per-peer state
/// has been evicted from the stack.
struct UdpRecvFuture {
    peer: SocketAddrV4,
    inner: Rc<RefCell<UdpStackInner>>,
}

impl Future for UdpRecvFuture {
    type Output = io::Result<Option<BytesMut>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.inner.borrow_mut();
        let state = match inner.connections.get_mut(&self.peer) {
            Some(s) => s,
            None => {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    format!("peer {} dropped from stack state", self.peer),
                )));
            }
        };
        if let Some(frame) = state.rx_queue.pop_front() {
            return Poll::Ready(Ok(Some(frame)));
        }
        if state.closed {
            return Poll::Ready(Ok(None));
        }
        // Park: register our waker so `XdpStack::on_rx` (or `Drop`) wakes
        // us when a frame is enqueued or the connection is closed.
        state.rx_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl LocalConnection for UdpConnection {
    type Error = io::Error;

    fn recv(&mut self) -> impl Future<Output = io::Result<Option<BytesMut>>> + '_ {
        UdpRecvFuture {
            peer: self.peer,
            inner: Rc::clone(&self.inner),
        }
    }

    async fn send(&mut self, msg: &[u8]) -> io::Result<()> {
        let owned = Bytes::copy_from_slice(msg);
        self.send_owned(owned).await
    }

    async fn send_owned(&mut self, msg: Bytes) -> io::Result<()> {
        let mut inner = self.inner.borrow_mut();
        let frame = inner
            .build_outbound(self.peer, &msg)
            .map_err(|e| io::Error::other(e.to_string()))?;
        inner.pending_tx.push_back(frame);
        Ok(())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::V4(self.peer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_MAC: MacAddr = [0x02, 0, 0, 0, 0, 0xaa];
    const PEER_MAC: MacAddr = [0x02, 0, 0, 0, 0, 0xbb];
    const LOCAL_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
    const PEER_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);

    fn make_stack() -> UdpStack {
        UdpStack::new(UdpStackConfig::new(LOCAL_IP, 9000, LOCAL_MAC).max_frame_size(4096))
    }

    /// Builds an inbound UDP frame from peer to local with a length-prefixed
    /// payload, ready to be fed into `on_rx`.
    fn inbound_frame(payload: &[u8]) -> Vec<u8> {
        let frame_len_u32 = payload.len() as u32;
        let mut datagram = Vec::with_capacity(4 + payload.len());
        datagram.extend_from_slice(&frame_len_u32.to_le_bytes());
        datagram.extend_from_slice(payload);
        build_udp_ipv4(
            PEER_MAC, LOCAL_MAC, PEER_IP, LOCAL_IP, 5555, 9000, &datagram,
        )
        .expect("build inbound")
    }

    #[test]
    fn test_first_frame_creates_connection_and_emits_payload() {
        let mut stack = make_stack();
        let mut tx = Vec::new();
        let mut q = FrameTxQueue::new(&mut tx);
        let frame = inbound_frame(b"hello");
        let conn = stack.on_rx(&frame, &mut q).expect("on_rx ok");
        assert!(conn.is_some(), "first frame should accept a new connection");
        assert!(tx.is_empty(), "no outbound traffic on rx");
    }

    #[test]
    fn test_second_frame_from_same_peer_reuses_connection() {
        let mut stack = make_stack();
        let mut tx = Vec::new();
        let mut q = FrameTxQueue::new(&mut tx);
        let frame1 = inbound_frame(b"first");
        let frame2 = inbound_frame(b"second");
        let conn = stack.on_rx(&frame1, &mut q).expect("on_rx 1");
        assert!(conn.is_some());
        let conn = stack.on_rx(&frame2, &mut q).expect("on_rx 2");
        assert!(conn.is_none(), "second frame must not produce a new conn");
    }

    #[test]
    fn test_frame_to_wrong_port_is_dropped() {
        let mut stack = make_stack();
        let mut tx = Vec::new();
        let mut q = FrameTxQueue::new(&mut tx);
        let datagram = {
            let mut d = Vec::new();
            d.extend_from_slice(&1u32.to_le_bytes());
            d.push(b'x');
            d
        };
        let frame = build_udp_ipv4(
            PEER_MAC, LOCAL_MAC, PEER_IP, LOCAL_IP, 5555, 9999, &datagram,
        )
        .expect("build");
        let conn = stack.on_rx(&frame, &mut q).expect("on_rx");
        assert!(conn.is_none());
    }

    #[test]
    fn test_arp_request_for_local_ip_emits_reply() {
        use etherparse::{EtherType, Ethernet2Header};
        // Hand-roll an ARP request (RFC 826) so the test does not depend
        // on etherparse's evolving ARP types.
        let eth = Ethernet2Header {
            source: PEER_MAC,
            destination: [0xff; 6],
            ether_type: EtherType::ARP,
        };
        let mut frame = Vec::new();
        eth.write(&mut frame).expect("eth");
        frame.extend_from_slice(&1u16.to_be_bytes()); // HTYPE
        frame.extend_from_slice(&0x0800u16.to_be_bytes()); // PTYPE
        frame.push(6); // HLEN
        frame.push(4); // PLEN
        frame.extend_from_slice(&1u16.to_be_bytes()); // OPER = request
        frame.extend_from_slice(&PEER_MAC);
        frame.extend_from_slice(&PEER_IP.octets());
        frame.extend_from_slice(&[0u8; 6]);
        frame.extend_from_slice(&LOCAL_IP.octets());

        let mut stack = make_stack();
        let mut tx = Vec::new();
        let mut q = FrameTxQueue::new(&mut tx);
        let conn = stack.on_rx(&frame, &mut q).expect("on_rx");
        assert!(conn.is_none(), "arp must not produce a Connection");
        assert_eq!(tx.len(), 1, "arp request must produce exactly one reply");
    }

    #[test]
    fn test_send_owned_then_accept_and_recv_round_trip() {
        // Synchronous round-trip: feed one inbound frame, accept the
        // connection, recv the payload, send a response, and verify the
        // outbound frame appears in pending_tx.
        let mut stack = make_stack();
        let mut tx = Vec::new();
        let mut q = FrameTxQueue::new(&mut tx);
        let frame = inbound_frame(b"hello");
        let mut conn = stack
            .on_rx(&frame, &mut q)
            .expect("on_rx")
            .expect("conn produced");

        // Drain rx via the LocalConnection trait method.  We poll the
        // future once because the implementation is synchronous.
        let fut = conn.recv();
        let received = futures_lite_poll(fut).expect("recv ok").expect("frame");
        assert_eq!(&received[..], b"hello");

        // Send a response and confirm it ended up in pending_tx.
        let send_fut = async { conn.send_owned(Bytes::from_static(b"world")).await };
        let send_res = futures_lite_poll(send_fut);
        assert!(send_res.is_ok(), "send_owned must succeed");
        stack.drain_pending_tx(&mut q);
        assert_eq!(tx.len(), 1, "exactly one outbound frame should be queued");
    }

    /// Synchronously poll a future that is known not to actually `.await`
    /// anything (because the UDP stack is fully synchronous).  Used in
    /// tests to avoid pulling in tokio just for `block_on`.
    fn futures_lite_poll<T>(fut: impl std::future::Future<Output = T>) -> T {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut fut = pin!(fut);
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("UdpStack futures must complete synchronously in tests"),
        }
    }
}
