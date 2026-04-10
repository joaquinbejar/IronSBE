//! Build script that probes for `libdpdk` via `pkg-config`.
//!
//! `pkg-config` emits the correct `cargo:rustc-link-lib` and
//! `cargo:rustc-link-search` metadata automatically, so no manual
//! flag loops are needed.

fn main() {
    if let Err(e) = pkg_config::Config::new()
        .atleast_version("23.11")
        .probe("libdpdk")
    {
        panic!(
            "\n\nironsbe-transport-dpdk requires DPDK >= 23.11.\n\
             Install it with:\n\n\
             \tsudo apt-get install -y libdpdk-dev\n\n\
             pkg-config error: {e}\n"
        );
    }
}
