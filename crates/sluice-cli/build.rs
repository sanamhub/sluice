//! Gives the crate the target triple it is built for, which `sluice doctor` prints. Cargo sets
//! `TARGET` for build scripts only.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=SLUICE_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
