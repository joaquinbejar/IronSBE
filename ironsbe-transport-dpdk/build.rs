//! Build script that:
//!
//! 1. Probes for `libdpdk` via `pkg-config`.
//! 2. Runs `bindgen` over a small wrapper header to generate Rust
//!    bindings for the DPDK types and functions we use.
//! 3. Compiles a tiny C shim (`shim.c`) that wraps macro-based DPDK
//!    accessors (`rte_pktmbuf_mtod`, `rte_pktmbuf_data_len`) as real
//!    functions callable from Rust.

use std::env;
use std::path::PathBuf;

fn main() {
    // 1. pkg-config probe — emits link flags automatically.
    let dpdk = match pkg_config::Config::new()
        .atleast_version("23.11")
        .probe("libdpdk")
    {
        Ok(lib) => lib,
        Err(e) => {
            panic!(
                "\n\nironsbe-transport-dpdk requires DPDK >= 23.11.\n\
                 Install it with:\n\n\
                 \tsudo apt-get install -y libdpdk-dev\n\n\
                 pkg-config error: {e}\n"
            );
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    // 2. bindgen — generate Rust bindings from the wrapper header.
    let mut builder = bindgen::Builder::default()
        .header("src/dpdk_wrapper.h")
        // Only generate bindings for the symbols we actually use.
        // This keeps the output small (~2k lines vs ~50k for all
        // DPDK headers).
        .allowlist_function("rte_eal_init")
        .allowlist_function("rte_eal_cleanup")
        .allowlist_function("rte_socket_id")
        .allowlist_function("rte_pktmbuf_pool_create")
        .allowlist_function("rte_mempool_free")
        .allowlist_function("rte_eth_dev_count_avail")
        .allowlist_function("rte_eth_dev_configure")
        .allowlist_function("rte_eth_rx_queue_setup")
        .allowlist_function("rte_eth_tx_queue_setup")
        .allowlist_function("rte_eth_dev_start")
        .allowlist_function("rte_eth_dev_stop")
        .allowlist_function("rte_eth_dev_close")
        .allowlist_function("rte_eth_rx_burst")
        .allowlist_function("rte_eth_tx_burst")
        .allowlist_function("rte_eth_promiscuous_enable")
        .allowlist_function("rte_pktmbuf_alloc")
        .allowlist_function("rte_pktmbuf_free")
        .allowlist_function("rte_pktmbuf_append")
        // Also generate the struct types these functions reference.
        .allowlist_type("rte_eth_conf")
        .allowlist_type("rte_mbuf")
        .allowlist_type("rte_mempool")
        .allowlist_type("rte_eth_dev_info")
        // Derive Default so we can zero-init rte_eth_conf etc.
        .derive_default(true)
        // Use core types for no_std compat (even though we use std).
        .use_core()
        .layout_tests(true)
        // Generate Rust 2021-compatible code so `unsafe fn` bodies
        // don't need explicit `unsafe {}` blocks (Rust 2024 changed
        // the default).
        .rust_edition(bindgen::RustEdition::Edition2021)
        // Don't emit doc comments from DPDK headers — they contain
        // free-form text that rustdoc tries to parse as doctests.
        .generate_comments(false);

    // Pass the full CFLAGS from pkg-config (include paths, arch flags,
    // `-include rte_config.h`, …) so clang sees exactly the same
    // struct layout as the C compiler.  Just `-I` paths are not enough
    // because config macros can change struct sizes.
    let cflags_bg = std::process::Command::new("pkg-config")
        .args(["--cflags", "libdpdk"])
        .output()
        .expect("failed to run pkg-config --cflags libdpdk for bindgen");
    if !cflags_bg.status.success() {
        let stderr = String::from_utf8_lossy(&cflags_bg.stderr);
        panic!(
            "pkg-config --cflags libdpdk failed ({}): {}",
            cflags_bg.status,
            stderr.trim()
        );
    }
    for flag in String::from_utf8_lossy(&cflags_bg.stdout).split_whitespace() {
        builder = builder.clang_arg(flag);
    }

    let bindings = builder
        .generate()
        .expect("bindgen failed to generate DPDK bindings");
    bindings
        .write_to_file(out_dir.join("dpdk_bindings.rs"))
        .expect("failed to write dpdk_bindings.rs");

    // 3. Compile the C shim.
    //
    // DPDK headers use architecture-specific intrinsics (SSE/AVX in
    // rte_memcpy, etc.) and require `-include rte_config.h` plus
    // arch flags like `-march=corei7`.  We extract these from the
    // pkg-config CFLAGS and pass them through to `cc`.
    let mut cc_build = cc::Build::new();
    cc_build.file("src/shim.c");
    for path in &dpdk.include_paths {
        cc_build.include(path);
    }
    // Forward all CFLAGS that pkg-config advertises.  This includes
    // `-include rte_config.h`, `-march=corei7`, `-mrtm`, etc.
    let cflags_output = std::process::Command::new("pkg-config")
        .args(["--cflags", "libdpdk"])
        .output()
        .expect("failed to run pkg-config --cflags libdpdk");
    if !cflags_output.status.success() {
        let stderr = String::from_utf8_lossy(&cflags_output.stderr);
        panic!(
            "pkg-config --cflags libdpdk failed ({}): {}",
            cflags_output.status,
            stderr.trim()
        );
    }
    let cflags = String::from_utf8_lossy(&cflags_output.stdout);
    for flag in cflags.split_whitespace() {
        cc_build.flag(flag);
    }
    cc_build.compile("ironsbe_dpdk_shim");

    println!("cargo:rerun-if-changed=src/dpdk_wrapper.h");
    println!("cargo:rerun-if-changed=src/shim.c");
}
