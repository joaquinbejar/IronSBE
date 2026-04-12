//! High-level [`LocalTransport`] implementation for the RDMA backend.

use crate::connection::RdmaConnection;
use crate::listener::RdmaListener;
use ironsbe_transport::traits::LocalTransport;
use std::io;
use std::net::SocketAddr;

/// RDMA transport configuration.
///
/// Used for both the listener (bind) and client (connect) sides of
/// an RDMA CM connection.  The [`addr`](Self::addr) field is the bind
/// address for [`LocalTransport::bind_with`] and the remote target
/// for [`LocalTransport::connect_with`].
#[derive(Debug, Clone)]
pub struct RdmaConfig {
    /// Target address — the bind address for listeners, the remote
    /// endpoint for client connections.
    pub addr: SocketAddr,
    /// Maximum SBE message size in bytes (excluding the 4-byte length
    /// prefix).
    pub max_msg_size: usize,
}

impl RdmaConfig {
    /// Creates a new RDMA config.
    #[must_use]
    pub fn new(addr: SocketAddr, max_msg_size: usize) -> Self {
        Self { addr, max_msg_size }
    }
}

impl From<SocketAddr> for RdmaConfig {
    fn from(addr: SocketAddr) -> Self {
        Self {
            addr,
            max_msg_size: 64 * 1024,
        }
    }
}

/// RDMA transport backend.
///
/// Implements [`LocalTransport`] using ibverbs Queue Pairs over RDMA
/// CM.  Works with any RDMA-capable NIC (InfiniBand, RoCE) or the
/// SoftRoCE (`rxe`) kernel module for testing.
pub struct RdmaTransport;

impl LocalTransport for RdmaTransport {
    type Listener = RdmaListener;
    type Connection = RdmaConnection;
    type Error = io::Error;
    type BindConfig = RdmaConfig;
    type ConnectConfig = RdmaConfig;

    async fn bind_with(config: RdmaConfig) -> io::Result<RdmaListener> {
        RdmaListener::bind(config.addr, config.max_msg_size)
    }

    async fn connect_with(config: RdmaConfig) -> io::Result<RdmaConnection> {
        RdmaConnection::connect(config.addr, config.max_msg_size).await
    }
}
