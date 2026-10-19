//! Exposes the locked iroh and iroh-blobs versions so a result line names what was measured.

use std::{env, fs, path::Path};

fn locked_version(lock: &str, name: &str) -> String {
    let header = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == header
            && let Some(version) = lines
                .next()
                .and_then(|l| l.strip_prefix("version = \""))
                .and_then(|l| l.strip_suffix('"'))
        {
            return version.to_string();
        }
    }
    panic!("{name} is not in Cargo.lock");
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let lock_path = Path::new(&manifest_dir).join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let lock = fs::read_to_string(&lock_path).expect("read the workspace Cargo.lock");
    println!(
        "cargo:rustc-env=PROBE_IROH_VERSION={}",
        locked_version(&lock, "iroh")
    );
    println!(
        "cargo:rustc-env=PROBE_IROH_BLOBS_VERSION={}",
        locked_version(&lock, "iroh-blobs")
    );
}
