//! Build script: probes for libibverbs + librdmacm via pkg-config,
//! generates Rust bindings via bindgen.

use std::env;
use std::path::PathBuf;

fn main() {
    // Probe for the two required libraries.
    let verbs = pkg_config::Config::new()
        .probe("libibverbs")
        .unwrap_or_else(|e| {
            panic!(
                "\n\nironsbe-transport-rdma requires libibverbs.\n\
                 Install with: sudo apt-get install -y libibverbs-dev\n\n\
                 pkg-config error: {e}\n"
            );
        });

    let _rdmacm = pkg_config::Config::new()
        .probe("librdmacm")
        .unwrap_or_else(|e| {
            panic!(
                "\n\nironsbe-transport-rdma requires librdmacm.\n\
                 Install with: sudo apt-get install -y librdmacm-dev\n\n\
                 pkg-config error: {e}\n"
            );
        });

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    // bindgen over the wrapper header.
    let mut builder = bindgen::Builder::default()
        .header("src/rdma_wrapper.h")
        // Only the types/functions we actually use.
        .allowlist_function("rdma_create_event_channel")
        .allowlist_function("rdma_destroy_event_channel")
        .allowlist_function("rdma_create_id")
        .allowlist_function("rdma_destroy_id")
        .allowlist_function("rdma_bind_addr")
        .allowlist_function("rdma_listen")
        .allowlist_function("rdma_accept")
        .allowlist_function("rdma_connect")
        .allowlist_function("rdma_disconnect")
        .allowlist_function("rdma_get_cm_event")
        .allowlist_function("rdma_ack_cm_event")
        .allowlist_function("rdma_get_request")
        .allowlist_function("rdma_create_qp")
        .allowlist_function("rdma_destroy_qp")
        .allowlist_function("ibv_reg_mr")
        .allowlist_function("ibv_dereg_mr")
        .allowlist_function("ibv_post_send")
        .allowlist_function("ibv_post_recv")
        .allowlist_function("ibv_poll_cq")
        .allowlist_function("ibv_create_cq")
        .allowlist_function("ibv_destroy_cq")
        .allowlist_function("ibv_alloc_pd")
        .allowlist_function("ibv_dealloc_pd")
        .allowlist_type("rdma_cm_id")
        .allowlist_type("rdma_event_channel")
        .allowlist_type("rdma_cm_event")
        .allowlist_type("rdma_conn_param")
        .allowlist_type("ibv_qp_init_attr")
        .allowlist_type("ibv_send_wr")
        .allowlist_type("ibv_recv_wr")
        .allowlist_type("ibv_sge")
        .allowlist_type("ibv_wc")
        .allowlist_type("ibv_mr")
        .allowlist_type("ibv_pd")
        .allowlist_type("ibv_cq")
        .derive_default(true)
        .use_core()
        .layout_tests(true)
        .rust_edition(bindgen::RustEdition::Edition2021)
        .generate_comments(false);

    for path in &verbs.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed to generate RDMA bindings");
    bindings
        .write_to_file(out_dir.join("rdma_bindings.rs"))
        .expect("failed to write rdma_bindings.rs");

    println!("cargo:rerun-if-changed=src/rdma_wrapper.h");
}
