mod args;
mod context;
mod examples;
mod files;
mod handover;
mod keys;
mod rendezvous;
mod rules;
mod secrets;
mod settings;
mod tokens;
mod versions;

use std::path::{Path, PathBuf};

/// The `docker` folder, which holds the guard and both images' folders.
fn docker_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the guard lives in docker/guard")
}

/// The `docker/relay` folder, which holds the relay's shipped configs and its Dockerfile.
fn relay_dir() -> PathBuf {
    docker_dir().join("relay")
}

/// The `docker/rendezvous` folder, which holds the rendezvous config and its Dockerfile.
fn rendezvous_dir() -> PathBuf {
    docker_dir().join("rendezvous")
}

/// The repository root, which holds the Cargo.lock of the probe and the workspace.
fn repo_root() -> PathBuf {
    docker_dir().join("..")
}

/// `body` after `enable_metrics = false`, which every config must write out (ADR 0020), and with a
/// `[tls]` table unless the body writes one, so that a test of another rule is not refused for the
/// rule of ADR 0021 first. A table added at the end moves no line of the body.
fn m(body: &str) -> String {
    let tls = match body.contains("[tls]") {
        true => "",
        false => "[tls]\ncert_mode = \"Manual\"\n",
    };
    format!("enable_metrics = false\n{body}{tls}")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}
