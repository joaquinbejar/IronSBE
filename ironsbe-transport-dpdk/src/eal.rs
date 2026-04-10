//! DPDK EAL (Environment Abstraction Layer) initialization and cleanup.
//!
//! The EAL must be initialized exactly once per process before any
//! DPDK function can be called.  [`Eal::init`] wraps
//! `rte_eal_init` and [`Eal`]'s `Drop` calls `rte_eal_cleanup`.

use crate::ffi;
use std::ffi::{CString, NulError};
use std::io;

/// RAII guard for the DPDK EAL.
///
/// Created by [`Eal::init`].  When dropped, calls `rte_eal_cleanup`
/// to release EAL resources.  Only one instance may exist per process.
pub struct Eal {
    /// Owned CStrings backing the argv pointers so they live as long
    /// as EAL might reference them (DPDK stores pointers internally).
    _args: Vec<CString>,
}

impl Eal {
    /// Initializes the DPDK EAL with the given command-line arguments.
    ///
    /// For testing with the AF_XDP PMD on loopback without hugepages:
    ///
    /// ```text
    /// ["ironsbe", "--no-huge", "--proc-type=primary",
    ///  "--vdev=net_af_xdp0,iface=eth0,start_queue=0,queue_count=1"]
    /// ```
    ///
    /// # Errors
    /// Returns an `io::Error` if EAL initialization fails (missing
    /// capabilities, invalid args, …).
    pub fn init(args: &[&str]) -> io::Result<Self> {
        let c_args: Vec<CString> = args
            .iter()
            .map(|s| CString::new(*s))
            .collect::<Result<Vec<_>, NulError>>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let mut ptrs: Vec<*mut i8> = c_args.iter().map(|s| s.as_ptr() as *mut i8).collect();

        // SAFETY: we pass valid C strings whose lifetimes outlive
        // this call (stored in `_args` on success).  EAL init is
        // required to be called exactly once per process.
        let ret = unsafe { ffi::rte_eal_init(ptrs.len() as i32, ptrs.as_mut_ptr()) };

        if ret < 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("rte_eal_init failed with code {ret}"),
            ));
        }

        tracing::info!(parsed_args = ret, "DPDK EAL initialized");
        Ok(Self { _args: c_args })
    }
}

impl Drop for Eal {
    fn drop(&mut self) {
        // SAFETY: EAL was successfully initialized in `init`.
        unsafe {
            ffi::rte_eal_cleanup();
        }
        tracing::info!("DPDK EAL cleaned up");
    }
}
