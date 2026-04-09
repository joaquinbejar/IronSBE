//! Ethernet / IPv4 / UDP / ARP frame builders and parsers used by the
//! AF_XDP datapath and the user-space stacks above it.
//!
//! All helpers operate on borrowed `&[u8]` / `&mut [u8]` so the caller can
//! drive UMEM-resident frames without copies.  No allocations on the hot
//! path.

use etherparse::{EtherType, Ethernet2Header, PacketBuilder, SlicedPacket, TransportSlice};
use std::net::Ipv4Addr;

/// On-wire size of an ARP-over-Ethernet packet (RFC 826).
const ARP_PACKET_LEN: usize = 28;
/// EtherType for ARP.
const ETHERTYPE_ARP: u16 = 0x0806;

/// Errors produced when parsing or building network frames.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// Frame was too short or malformed.
    #[error("malformed frame: {0}")]
    Malformed(String),
    /// Frame referred to a higher layer we don't speak.
    #[error("unsupported protocol")]
    Unsupported,
    /// Buffer too small to hold the frame being built.
    #[error("buffer too small for frame: needed {needed}, got {got}")]
    BufferTooSmall {
        /// Minimum bytes required.
        needed: usize,
        /// Bytes available.
        got: usize,
    },
}

/// A 6-byte hardware (Ethernet) address.
pub type MacAddr = [u8; 6];

/// Parsed view of an inbound Ethernet frame.
#[derive(Debug)]
pub struct ParsedFrame<'a> {
    /// Source MAC.
    pub src_mac: MacAddr,
    /// Destination MAC.
    pub dst_mac: MacAddr,
    /// EtherType (e.g. `ETH_P_IP`, `ETH_P_ARP`).
    pub ether_type: u16,
    /// Layer-3+ payload, after the Ethernet header.
    pub l3_payload: &'a [u8],
}

/// Parsed view of an inbound UDP/IPv4 frame.
#[derive(Debug)]
pub struct ParsedUdp<'a> {
    /// Source IPv4 address.
    pub src_ip: Ipv4Addr,
    /// Destination IPv4 address.
    pub dst_ip: Ipv4Addr,
    /// Source UDP port.
    pub src_port: u16,
    /// Destination UDP port.
    pub dst_port: u16,
    /// UDP payload.
    pub payload: &'a [u8],
}

/// Parsed view of an inbound ARP request.
#[derive(Debug)]
pub struct ParsedArp {
    /// Sender hardware address.
    pub sender_mac: MacAddr,
    /// Sender protocol address.
    pub sender_ip: Ipv4Addr,
    /// Target protocol address.
    pub target_ip: Ipv4Addr,
    /// Operation: 1 = request, 2 = reply.
    pub operation: u16,
}

/// Parses the Ethernet header of an inbound frame.
///
/// # Errors
/// Returns [`FrameError::Malformed`] if the frame is too short.
#[inline]
pub fn parse_ethernet(frame: &[u8]) -> Result<ParsedFrame<'_>, FrameError> {
    let (eth, rest) =
        Ethernet2Header::from_slice(frame).map_err(|e| FrameError::Malformed(e.to_string()))?;
    Ok(ParsedFrame {
        src_mac: eth.source,
        dst_mac: eth.destination,
        ether_type: u16::from(eth.ether_type),
        l3_payload: rest,
    })
}

/// Parses an IPv4/UDP frame end-to-end.
///
/// Returns `Ok(None)` if the frame is well-formed but not UDP/IPv4 (e.g.
/// TCP, ICMP, IPv6) so callers can dispatch on protocol.
///
/// # Errors
/// Returns [`FrameError::Malformed`] if the frame fails to parse.
#[inline]
pub fn parse_ipv4_udp(frame: &[u8]) -> Result<Option<ParsedUdp<'_>>, FrameError> {
    let sliced =
        SlicedPacket::from_ethernet(frame).map_err(|e| FrameError::Malformed(e.to_string()))?;
    let net = match sliced.net {
        Some(etherparse::NetSlice::Ipv4(ip)) => ip,
        _ => return Ok(None),
    };
    let header = net.header();
    let src_ip = Ipv4Addr::from(header.source());
    let dst_ip = Ipv4Addr::from(header.destination());
    let transport = match sliced.transport {
        Some(TransportSlice::Udp(udp)) => udp,
        _ => return Ok(None),
    };
    Ok(Some(ParsedUdp {
        src_ip,
        dst_ip,
        src_port: transport.source_port(),
        dst_port: transport.destination_port(),
        payload: transport.payload(),
    }))
}

/// Parses an Ethernet+ARP frame.
///
/// Returns `Ok(None)` if the EtherType is not ARP.
///
/// # Errors
/// Returns [`FrameError::Malformed`] if parsing fails (header too short,
/// unsupported HTYPE/PTYPE, …).
pub fn parse_arp(frame: &[u8]) -> Result<Option<ParsedArp>, FrameError> {
    let parsed = parse_ethernet(frame)?;
    if parsed.ether_type != ETHERTYPE_ARP {
        return Ok(None);
    }
    if parsed.l3_payload.len() < ARP_PACKET_LEN {
        return Err(FrameError::Malformed(format!(
            "arp payload {} < {}",
            parsed.l3_payload.len(),
            ARP_PACKET_LEN
        )));
    }
    let p = parsed.l3_payload;
    let htype = u16::from_be_bytes([p[0], p[1]]);
    let ptype = u16::from_be_bytes([p[2], p[3]]);
    let hlen = p[4];
    let plen = p[5];
    if htype != 1 || ptype != 0x0800 || hlen != 6 || plen != 4 {
        return Err(FrameError::Unsupported);
    }
    let operation = u16::from_be_bytes([p[6], p[7]]);
    let mut sender_mac = [0u8; 6];
    sender_mac.copy_from_slice(&p[8..14]);
    let sender_ip = Ipv4Addr::new(p[14], p[15], p[16], p[17]);
    let target_ip = Ipv4Addr::new(p[24], p[25], p[26], p[27]);
    Ok(Some(ParsedArp {
        sender_mac,
        sender_ip,
        target_ip,
        operation,
    }))
}

/// Builds an Ethernet+IPv4+UDP datagram into an owned `Vec<u8>`.
///
/// The returned vector is the full on-wire frame ready to be pushed into a
/// TX ring slot.
///
/// # Errors
/// Returns [`FrameError::Malformed`] if `etherparse` rejects the inputs
/// (oversized payload, header layout problem, …).  Note that the helper
/// does not currently distinguish "payload too large" from other framing
/// problems; callers that need finer-grained errors should validate the
/// payload length up-front.
pub fn build_udp_ipv4(
    src_mac: MacAddr,
    dst_mac: MacAddr,
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Result<Vec<u8>, FrameError> {
    let builder = PacketBuilder::ethernet2(src_mac, dst_mac)
        .ipv4(src_ip.octets(), dst_ip.octets(), 64)
        .udp(src_port, dst_port);
    let mut out = Vec::with_capacity(builder.size(payload.len()));
    builder
        .write(&mut out, payload)
        .map_err(|e| FrameError::Malformed(e.to_string()))?;
    Ok(out)
}

/// Builds an Ethernet+ARP reply frame.
///
/// Used by the [`crate::xdp::stack::UdpStack`] to answer ARP probes for
/// the bound IP so peers can resolve our MAC.  Hand-rolled (RFC 826) so
/// we don't depend on `etherparse` evolving its ARP API.
///
/// # Errors
/// Returns [`FrameError::Malformed`] on serialisation failure.
pub fn build_arp_reply(
    our_mac: MacAddr,
    our_ip: Ipv4Addr,
    target_mac: MacAddr,
    target_ip: Ipv4Addr,
) -> Result<Vec<u8>, FrameError> {
    let eth = Ethernet2Header {
        source: our_mac,
        destination: target_mac,
        ether_type: EtherType::ARP,
    };
    let mut out = Vec::with_capacity(eth.header_len() + ARP_PACKET_LEN);
    eth.write(&mut out)
        .map_err(|e| FrameError::Malformed(e.to_string()))?;
    // ARP fixed header (RFC 826).
    out.extend_from_slice(&1u16.to_be_bytes()); // HTYPE = Ethernet
    out.extend_from_slice(&0x0800u16.to_be_bytes()); // PTYPE = IPv4
    out.push(6); // HLEN
    out.push(4); // PLEN
    out.extend_from_slice(&2u16.to_be_bytes()); // OPER = reply
    out.extend_from_slice(&our_mac); // SHA
    out.extend_from_slice(&our_ip.octets()); // SPA
    out.extend_from_slice(&target_mac); // THA
    out.extend_from_slice(&target_ip.octets()); // TPA
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC_MAC: MacAddr = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    const DST_MAC: MacAddr = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
    const SRC_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
    const DST_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);

    #[test]
    fn test_round_trip_udp_ipv4() {
        let payload = b"hello sbe over xdp";
        let frame =
            build_udp_ipv4(SRC_MAC, DST_MAC, SRC_IP, DST_IP, 9000, 9100, payload).expect("build");
        let parsed = parse_ipv4_udp(&frame)
            .expect("parse ok")
            .expect("udp packet");
        assert_eq!(parsed.src_ip, SRC_IP);
        assert_eq!(parsed.dst_ip, DST_IP);
        assert_eq!(parsed.src_port, 9000);
        assert_eq!(parsed.dst_port, 9100);
        assert_eq!(parsed.payload, payload);
    }

    #[test]
    fn test_parse_ethernet_short_frame_errors() {
        let too_short = [0u8; 5];
        let result = parse_ethernet(&too_short);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_non_udp_returns_none() {
        // ICMP echo request packet (Ethernet + IPv4 + ICMP).
        let builder = PacketBuilder::ethernet2(SRC_MAC, DST_MAC)
            .ipv4(SRC_IP.octets(), DST_IP.octets(), 64)
            .icmpv4_echo_request(0, 0);
        let mut frame = Vec::new();
        builder.write(&mut frame, &[]).expect("write icmp");
        let parsed = parse_ipv4_udp(&frame).expect("parse ok");
        assert!(parsed.is_none(), "icmp should not be reported as udp");
    }

    #[test]
    fn test_arp_reply_round_trip() {
        let reply = build_arp_reply(DST_MAC, DST_IP, SRC_MAC, SRC_IP).expect("build arp reply");
        let parsed = parse_arp(&reply).expect("parse ok").expect("arp packet");
        assert_eq!(parsed.operation, 2);
        assert_eq!(parsed.sender_mac, DST_MAC);
        assert_eq!(parsed.sender_ip, DST_IP);
        assert_eq!(parsed.target_ip, SRC_IP);
    }

    #[test]
    fn test_parse_arp_on_non_arp_returns_none() {
        let payload = b"x";
        let frame = build_udp_ipv4(SRC_MAC, DST_MAC, SRC_IP, DST_IP, 1, 2, payload).expect("build");
        let parsed = parse_arp(&frame).expect("parse ok");
        assert!(parsed.is_none());
    }
}
