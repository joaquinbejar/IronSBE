//! DPDK EAL (Environment Abstraction Layer) initialization and cleanup.
//!
//! The EAL must be initialized exactly once per process before any
//! DPDK function can be called.  [`Eal::init`] wraps
//! `rte_eal_init` and [`Eal`]'s `Drop` calls `rte_eal_cleanup`.

use crate::ffi;
use std::ffi::{CString, NulError};
use std::io;
use std::sync::{Arc, OnceLock};

/// Global EAL state — ensures `rte_eal_init` is called exactly once
/// per process, and `rte_eal_cleanup` only runs when the last
/// [`Eal`] handle is dropped.
static EAL_INNER: OnceLock<Arc<EalInner>> = OnceLock::new();

struct EalInner {
    /// Owned CStrings backing the argv pointers so they live as long
    /// as EAL might reference them (DPDK stores pointers internally).
    _args: Vec<CString>,
}

impl Drop for EalInner {
    fn drop(&mut self) {
        // SAFETY: EAL was successfully initialized.
        unsafe {
            ffi::rte_eal_cleanup();
        }
        tracing::info!("DPDK EAL cleaned up");
    }
}

/// RAII guard for the DPDK EAL.
///
/// Created by [`Eal::init`].  Multiple handles can exist (via
/// `Clone`); `rte_eal_cleanup` is called only when the **last**
/// handle is dropped.  The first call to [`init`](Self::init)
/// performs `rte_eal_init`; subsequent calls return a shared handle
/// to the already-initialized EAL.
#[derive(Clone)]
pub struct Eal {
    _inner: Arc<EalInner>,
}

impl Eal {
    /// Initializes the DPDK EAL with the given command-line arguments.
    ///
    /// If EAL has already been initialized in this process, subsequent
    /// calls return a shared handle without calling `rte_eal_init`
    /// again (DPDK's "init exactly once" rule).
    ///
    /// For testing with the AF_XDP PMD on loopback without hugepages:
    ///
    /// ```text
    /// ["ironsbe", "--no-huge", "--proc-type=primary",
    ///  "--vdev=net_af_xdp0,iface=eth0,start_queue=0,queue_count=1"]
    /// ```
    ///
    /// # Errors
    /// Returns an `io::Error` if the first EAL initialization fails
    /// (missing capabilities, invalid args, …).
    pub fn init(args: &[&str]) -> io::Result<Self> {
        // If already initialized, return a shared handle.
        if let Some(inner) = EAL_INNER.get() {
            return Ok(Self {
                _inner: Arc::clone(inner),
            });
        }

        let c_args: Vec<CString> = args
            .iter()
            .map(|s| CString::new(*s))
            .collect::<Result<Vec<_>, NulError>>()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let mut ptrs: Vec<*mut i8> = c_args.iter().map(|s| s.as_ptr() as *mut i8).collect();

        // SAFETY: we pass valid C strings whose lifetimes outlive
        // this call (stored in `_args` on success).  OnceLock
        // guarantees this runs at most once.
        let ret = unsafe { ffi::rte_eal_init(ptrs.len() as i32, ptrs.as_mut_ptr()) };

        if ret < 0 {
            return Err(io::Error::other(format!(
                "rte_eal_init failed with code {ret}"
            )));
        }

        tracing::info!(parsed_args = ret, "DPDK EAL initialized");
        let inner = Arc::new(EalInner { _args: c_args });
        // If another thread raced us, OnceLock handles dedup; the
        // extra EalInner drops harmlessly (cleanup already done).
        let shared = EAL_INNER.get_or_init(|| inner);
        Ok(Self {
            _inner: Arc::clone(shared),
        })
    }
}
