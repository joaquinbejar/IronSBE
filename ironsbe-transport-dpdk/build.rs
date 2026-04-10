//! Build script that probes for `libdpdk` via `pkg-config`.
//!
//! If DPDK is not installed, the build fails with a clear message
//! pointing the user at the install instructions.

fn main() {
    match pkg_config::Config::new()
        .atleast_version("23.11")
        .probe("libdpdk")
    {
        Ok(lib) => {
            for path in &lib.link_paths {
                println!("cargo:rustc-link-search=native={}", path.display());
            }
            for lib_name in &lib.libs {
                println!("cargo:rustc-link-lib={lib_name}");
            }
        }
        Err(e) => {
            panic!(
                "\n\nironsbe-transport-dpdk requires DPDK >= 23.11.\n\
                 Install it with:\n\n\
                 \tsudo apt-get install -y libdpdk-dev\n\n\
                 pkg-config error: {e}\n"
            );
        }
    }
}
