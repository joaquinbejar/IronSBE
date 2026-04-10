//! Bindgen-generated RDMA/ibverbs FFI bindings.
//!
//! Generated at build time from `rdma/rdma_cma.h` and
//! `infiniband/verbs.h` installed via `libibverbs-dev` +
//! `librdmacm-dev`.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unsafe_op_in_unsafe_fn,
    clippy::useless_transmute,
    clippy::unnecessary_cast,
    clippy::too_many_arguments,
    clippy::redundant_static_lifetimes,
    clippy::derivable_impls,
    clippy::missing_safety_doc,
    clippy::ptr_offset_with_cast
)]

include!(concat!(env!("OUT_DIR"), "/rdma_bindings.rs"));

// =====================================================================
// C shim functions wrapping inline ibverbs functions.
// =====================================================================
unsafe extern "C" {
    pub fn ironsbe_ibv_post_send(
        qp: *mut ibv_qp,
        wr: *mut ibv_send_wr,
        bad_wr: *mut *mut ibv_send_wr,
    ) -> ::core::ffi::c_int;
    pub fn ironsbe_ibv_post_recv(
        qp: *mut ibv_qp,
        wr: *mut ibv_recv_wr,
        bad_wr: *mut *mut ibv_recv_wr,
    ) -> ::core::ffi::c_int;
    pub fn ironsbe_ibv_poll_cq(
        cq: *mut ibv_cq,
        num_entries: ::core::ffi::c_int,
        wc: *mut ibv_wc,
    ) -> ::core::ffi::c_int;
}

// =====================================================================
// Constants not picked up by bindgen (enum values / #defines).
// =====================================================================

/// `IBV_ACCESS_LOCAL_WRITE` — allow local writes to registered MR.
pub const IBV_ACCESS_LOCAL_WRITE: u32 = 1;
/// `IBV_ACCESS_REMOTE_WRITE` — allow RDMA writes from remote.
pub const IBV_ACCESS_REMOTE_WRITE: u32 = 2;
/// `IBV_SEND_SIGNALED` — request a CQ completion entry for this send.
pub const IBV_SEND_SIGNALED: u32 = 1 << 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rdma_cm_id_is_not_zero_sized() {
        assert!(
            core::mem::size_of::<rdma_cm_id>() > 0,
            "rdma_cm_id must be a real struct"
        );
    }

    #[test]
    fn test_ibv_send_wr_is_not_zero_sized() {
        assert!(
            core::mem::size_of::<ibv_send_wr>() > 0,
            "ibv_send_wr must be a real struct"
        );
    }

    #[test]
    fn test_ibv_sge_is_not_zero_sized() {
        assert!(
            core::mem::size_of::<ibv_sge>() > 0,
            "ibv_sge must be a real struct"
        );
    }
}
