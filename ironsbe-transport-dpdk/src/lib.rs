//! # IronSBE DPDK Transport
//!
//! DPDK-based transport backend for IronSBE.  Uses the
//! [DPDK](https://www.dpdk.org/) poll-mode driver framework for
//! kernel-bypass packet I/O.
//!
//! **This crate is Linux-only and requires `libdpdk-dev` ≥ 23.11
//! installed on the build machine.**  It is NOT built by default in
//! the IronSBE workspace — opt in explicitly:
//!
//! ```sh
//! cargo build -p ironsbe-transport-dpdk
//! ```
//!
//! # AF_XDP PMD mode
//!
//! The recommended deployment for hosts with a single NIC uses DPDK's
//! `net_af_xdp` poll-mode driver, which creates a virtual DPDK port
//! backed by an AF_XDP socket on an existing kernel NIC.  The NIC
//! stays in the kernel (SSH/monitoring keep working) while the
//! datapath gets DPDK's burst API, mempool, and lcore model.
//!
//! ```text
//! EAL args: ["ironsbe", "--no-huge", "--proc-type=primary",
//!            "--vdev=net_af_xdp0,iface=eth0,start_queue=0,queue_count=1"]
//! ```
//!
//! # Native DPDK PMD mode
//!
//! For dedicated NICs bound to `vfio-pci`, omit the `--vdev` argument
//! and let EAL auto-detect the PCI device.  This gives the lowest
//! latency but requires hugepages, NIC unbinding, and core isolation.
//! See `docs/transport-backends.md` for the full operational checklist.

pub mod eal;
pub mod ffi;
pub mod port;
pub mod transport;

pub use eal::Eal;
pub use port::DpdkPort;
pub use transport::{DpdkConfig, DpdkListener, DpdkTransport};
