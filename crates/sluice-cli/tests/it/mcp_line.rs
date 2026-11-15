//! `sluice mcp` and a line of input longer than it takes.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::common::EXE;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// How long the test waits for the server, so a hang fails and does not stall.
const PATIENCE: Duration = Duration::from_secs(120);

/// A line of input past the limit is dropped and not buffered, and the server goes on to answer the
/// request after it.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_line_is_dropped_and_the_next_request_is_answered() {
    let dir = tempfile::tempdir().unwrap();
    let (home, root): (PathBuf, PathBuf) = (dir.path().join("home"), dir.path().join("root"));
    std::fs::create_dir(&root).unwrap();
    let mut child = tokio::process::Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["mcp", "--loopback", "--root"])
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut huge = "x".repeat(300 * 1024);
    huge.push('\n');
    input.write_all(huge.as_bytes()).await.unwrap();
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" }
        }
    });
    input
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .unwrap();
    let line = tokio::time::timeout(PATIENCE, output.next_line())
        .await
        .expect("the server answers in time")
        .unwrap()
        .expect("a line");
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["id"], 1, "{reply}");
    assert!(reply["result"].is_object(), "{reply}");
}
