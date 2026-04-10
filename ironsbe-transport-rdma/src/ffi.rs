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
    clippy::ptr_offset_with_cast,
)]

include!(concat!(env!("OUT_DIR"), "/rdma_bindings.rs"));

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
