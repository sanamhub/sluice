mod args;
mod examples;
mod files;
mod rules;
mod secrets;
mod versions;

use std::path::{Path, PathBuf};

/// The `docker/relay` folder, which holds the shipped configs and the Dockerfile.
fn relay_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the guard lives in docker/relay")
}

/// The repository root, which holds the Cargo.lock of the probe and the workspace.
fn repo_root() -> PathBuf {
    relay_dir().join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}
