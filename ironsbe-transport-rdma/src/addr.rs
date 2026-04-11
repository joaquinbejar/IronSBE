//! Socket-address conversion helpers for the RDMA backend.
//!
//! `rdma_cm_id` exposes source and destination endpoints as
//! `struct sockaddr` / `struct sockaddr_storage` unions.  These
//! helpers pull a `std::net::SocketAddr` out of such a pointer so
//! `local_addr()` / `peer_addr()` can report the real endpoint.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// Converts a raw `sockaddr` pointer into a [`SocketAddr`].
///
/// Inspects `sa_family` and casts to `sockaddr_in` or `sockaddr_in6`
/// accordingly.  Any other address family returns
/// [`io::ErrorKind::Unsupported`].
///
/// # Errors
/// Returns `io::ErrorKind::InvalidInput` if `sa` is null.
/// Returns `io::ErrorKind::Unsupported` if the family is neither
/// `AF_INET` nor `AF_INET6`.
///
/// # Safety
/// `sa` must point to a valid `sockaddr` whose underlying storage is
/// at least large enough for the declared family (16 bytes for
/// `sockaddr_in`, 28 bytes for `sockaddr_in6`).  In practice RDMA CM
/// always fills a full `sockaddr_storage` (128 bytes), so a pointer
/// into the `rdma_addr` union satisfies this trivially.
pub(crate) unsafe fn sockaddr_to_socket_addr(
    sa: *const libc::sockaddr,
) -> io::Result<SocketAddr> {
    if sa.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "null sockaddr pointer",
        ));
    }

    // SAFETY: non-null pointer into caller-provided storage; reading
    // `sa_family` is always safe because every concrete sockaddr
    // variant starts with that field.
    let family = unsafe { (*sa).sa_family };

    match i32::from(family) {
        libc::AF_INET => {
            // SAFETY: the caller guarantees AF_INET storage is at
            // least `size_of::<sockaddr_in>()` bytes.
            let sin = unsafe { &*(sa as *const libc::sockaddr_in) };
            // `sin_port` and `sin_addr.s_addr` are in network byte
            // order; translate to host order for `Ipv4Addr` / the
            // `SocketAddr` port.
            let port = u16::from_be(sin.sin_port);
            let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        libc::AF_INET6 => {
            // SAFETY: the caller guarantees AF_INET6 storage is at
            // least `size_of::<sockaddr_in6>()` bytes.
            let sin6 = unsafe { &*(sa as *const libc::sockaddr_in6) };
            let port = u16::from_be(sin6.sin6_port);
            let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
            Ok(SocketAddr::V6(SocketAddrV6::new(
                ip,
                port,
                u32::from_be(sin6.sin6_flowinfo),
                sin6.sin6_scope_id,
            )))
        }
        other => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported sockaddr family: {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_sockaddr_to_socket_addr_ipv4() {
        let mut sin: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        sin.sin_family = libc::AF_INET as libc::sa_family_t;
        sin.sin_port = 0x1234_u16.to_be();
        sin.sin_addr.s_addr = u32::from_be_bytes([1, 2, 3, 4]).to_be();

        let result = unsafe {
            sockaddr_to_socket_addr(&sin as *const libc::sockaddr_in as *const libc::sockaddr)
        };

        match result {
            Ok(SocketAddr::V4(v4)) => {
                assert_eq!(*v4.ip(), Ipv4Addr::new(1, 2, 3, 4));
                assert_eq!(v4.port(), 0x1234);
            }
            other => panic!("expected SocketAddr::V4, got {other:?}"),
        }
    }

    #[test]
    fn test_sockaddr_to_socket_addr_ipv6() {
        let mut sin6: libc::sockaddr_in6 = unsafe { std::mem::zeroed() };
        sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        sin6.sin6_port = 0xBEEF_u16.to_be();
        // ::1 (loopback)
        sin6.sin6_addr.s6_addr = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

        let result = unsafe {
            sockaddr_to_socket_addr(&sin6 as *const libc::sockaddr_in6 as *const libc::sockaddr)
        };

        match result {
            Ok(SocketAddr::V6(v6)) => {
                assert_eq!(IpAddr::V6(*v6.ip()), "::1".parse::<IpAddr>().expect("::1"));
                assert_eq!(v6.port(), 0xBEEF);
            }
            other => panic!("expected SocketAddr::V6, got {other:?}"),
        }
    }

    #[test]
    fn test_sockaddr_to_socket_addr_rejects_unsupported_family() {
        let mut sa: libc::sockaddr = unsafe { std::mem::zeroed() };
        sa.sa_family = libc::AF_UNIX as libc::sa_family_t;

        let result = unsafe { sockaddr_to_socket_addr(&sa) };

        match result {
            Err(err) if err.kind() == io::ErrorKind::Unsupported => {}
            other => panic!("expected Unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn test_sockaddr_to_socket_addr_rejects_null() {
        let result = unsafe { sockaddr_to_socket_addr(std::ptr::null()) };
        match result {
            Err(err) if err.kind() == io::ErrorKind::InvalidInput => {}
            other => panic!("expected InvalidInput error, got {other:?}"),
        }
    }
}
