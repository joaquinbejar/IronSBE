//! High-level [`LocalTransport`] implementation for the RDMA backend.

use crate::connection::RdmaConnection;
use crate::listener::RdmaListener;
use ironsbe_transport::traits::LocalTransport;
use std::io;
use std::net::SocketAddr;

/// RDMA transport configuration.
#[derive(Debug, Clone)]
pub struct RdmaConfig {
    /// Address to bind the RDMA CM listener to.
    pub bind_addr: SocketAddr,
    /// Maximum SBE message size in bytes (excluding the 4-byte length
    /// prefix).
    pub max_msg_size: usize,
}

impl RdmaConfig {
    /// Creates a new RDMA config.
    #[must_use]
    pub fn new(bind_addr: SocketAddr, max_msg_size: usize) -> Self {
        Self {
            bind_addr,
            max_msg_size,
        }
    }
}

impl From<SocketAddr> for RdmaConfig {
    fn from(addr: SocketAddr) -> Self {
        Self {
            bind_addr: addr,
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
        RdmaListener::bind(config.bind_addr, config.max_msg_size)
    }

    async fn connect_with(config: RdmaConfig) -> io::Result<RdmaConnection> {
        // Client-side RDMA connect — establish a QP to the remote.
        // For now return unsupported; the first priority is the
        // server (listener) side.
        let _ = config;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "RDMA client connect not yet implemented",
        ))
    }
}
