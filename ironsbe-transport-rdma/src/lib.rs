//! # IronSBE RDMA Transport
//!
//! RDMA/ibverbs-based transport backend for IronSBE.  Uses Queue Pairs
//! (QPs) over RDMA CM for two-sided `SEND`/`RECV` operations that
//! bypass the kernel's TCP/IP stack entirely.
//!
//! **This crate is Linux-only and requires `libibverbs-dev` +
//! `librdmacm-dev` installed on the build machine.**  It is NOT built
//! by default in the IronSBE workspace — opt in explicitly:
//!
//! ```sh
//! cargo build -p ironsbe-transport-rdma
//! ```
//!
//! # Hardware requirements
//!
//! - Any RDMA-capable NIC: InfiniBand HCA, RoCE-capable Ethernet NIC
//!   (Mellanox/NVIDIA ConnectX, Broadcom, etc.), or **SoftRoCE**
//!   (`rdma_rxe` kernel module) for testing on any NIC.
//! - `rdma-core` userspace libraries.
//!
//! # SoftRoCE setup (for testing without RDMA hardware)
//!
//! ```sh
//! sudo modprobe rdma_rxe
//! sudo rdma link add rxe0 type rxe netdev eth0
//! rdma link show  # should show rxe0 state ACTIVE
//! ```

#[cfg(not(target_os = "linux"))]
compile_error!(
    "ironsbe-transport-rdma is Linux-only. \
     It requires libibverbs-dev + librdmacm-dev."
);

pub mod connection;
pub mod ffi;
pub mod listener;
pub mod transport;

pub use connection::RdmaConnection;
pub use listener::RdmaListener;
pub use transport::{RdmaConfig, RdmaTransport};
