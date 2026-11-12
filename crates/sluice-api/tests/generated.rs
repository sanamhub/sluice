//! The generated code under `src/gen` is checked in, so building the workspace needs neither
//! `protoc` nor `buf`. This test compiles `proto/sluice/v1/control.proto` again, with a protobuf
//! compiler written in Rust that is a dev-dependency of this crate and so needs no program
//! installed, and fails when the checked-in copy is not what the schema produces.
//!
//! To rewrite the checked-in copy after a schema change, run the test with
//! `SLUICE_API_WRITE_GENERATED=1`.

use std::path::{Path, PathBuf};

const GENERATED: &str = "sluice.v1.rs";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A file's text with Windows line ends read as the Unix ones a checkout may have changed them to.
fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
}

/// Compiles the schema and writes the Rust it generates into `out`.
fn generate(out: &Path) {
    let proto = manifest_dir().join("../../proto");
    let set = protox::compile(["sluice/v1/control.proto"], [proto]).expect("the schema compiles");
    tonic_prost_build::configure()
        .build_transport(true)
        .out_dir(out)
        .compile_fds(set)
        .expect("the code generator runs");
}

#[test]
fn the_checked_in_code_is_what_the_schema_generates() {
    let fresh = tempfile::tempdir().unwrap();
    generate(fresh.path());
    let made = text(&fresh.path().join(GENERATED));
    let kept = manifest_dir().join("src/gen").join(GENERATED);
    if std::env::var_os("SLUICE_API_WRITE_GENERATED").is_some() {
        std::fs::write(&kept, &made).unwrap();
    }
    let checked_in = text(&kept);
    assert!(
        made == checked_in,
        "src/gen/{GENERATED} is out of date: run this test with SLUICE_API_WRITE_GENERATED=1"
    );
}
