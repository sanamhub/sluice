//! `sluice mcp` as processes, driven through standard input and output with the SDK's own client.
//! Every endpoint is on 127.0.0.1 only: the servers are started with `--loopback`.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{EXE, data};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

/// How long a test waits for a job, so a hang fails and does not stall.
const PATIENCE: Duration = Duration::from_secs(120);

type Client = RunningService<RoleClient, ()>;

struct Folders {
    _dir: tempfile::TempDir,
    outgoing: PathBuf,
    incoming: PathBuf,
    sender_home: PathBuf,
    receiver_home: PathBuf,
}

fn folders() -> Folders {
    let dir = tempfile::tempdir().unwrap();
    let outgoing = dir.path().join("outgoing");
    let incoming = dir.path().join("incoming");
    std::fs::create_dir(&outgoing).unwrap();
    std::fs::create_dir(&incoming).unwrap();
    Folders {
        sender_home: dir.path().join("sender-home"),
        receiver_home: dir.path().join("receiver-home"),
        outgoing,
        incoming,
        _dir: dir,
    }
}

/// Starts `sluice mcp` with `root` and its own home, on loopback, and connects to it.
async fn connect(home: &Path, root: &Path) -> Client {
    let transport =
        TokioChildProcess::new(tokio::process::Command::new(EXE).configure(|command| {
            command
                .env("SLUICE_HOME", home)
                .args(["mcp", "--loopback", "--root"])
                .arg(root);
        }))
        .expect("start sluice mcp");
    ().serve(transport).await.expect("initialize")
}

/// One tool call, whole.
async fn call_whole(client: &Client, tool: &'static str, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().unwrap_or_default();
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
        .await
        .expect("the call is answered")
}

/// One tool call, and the structured answer it gave.
async fn call(client: &Client, tool: &'static str, arguments: Value) -> Value {
    call_whole(client, tool, arguments)
        .await
        .structured_content
        .expect("a structured answer")
}

fn at(root: &Path, path: &str) -> Value {
    json!({ "root": root.to_str().unwrap(), "path": path })
}

/// Polls a job until `done` holds of its status, and returns that status.
async fn until(client: &Client, job: &str, done: impl Fn(&Value) -> bool) -> Value {
    let started = Instant::now();
    loop {
        let status = call(client, "job_status", json!({ "job_id": job })).await;
        if done(&status["result"]) {
            return status["result"].clone();
        }
        assert!(
            started.elapsed() < PATIENCE,
            "the job did not get there: {status}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn ended(status: &Value) -> bool {
    status["state"] != "running"
}

/// Starts a send and waits for its ticket, which `job_status` gives once the send is prepared.
async fn send(client: &Client, arguments: Value) -> (String, String) {
    let started = call(client, "send", arguments).await;
    assert_eq!(started["ok"], true, "{started}");
    let job = started["result"]["job_id"].as_str().unwrap().to_string();
    let status = until(client, &job, |s| !s["secret_ticket"].is_null() || ended(s)).await;
    let ticket = status["secret_ticket"]
        .as_str()
        .expect("a ticket")
        .to_string();
    (job, ticket)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_list_is_what_the_snapshot_says() {
    let f = folders();
    let client = connect(&f.sender_home, &f.outgoing).await;
    let mut tools: Vec<Value> = client
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    insta::assert_snapshot!(serde_json::to_string_pretty(&tools).unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_and_a_receive_between_two_servers_finish_verified() {
    let f = folders();
    let body = data(3 * 1024 * 1024 + 5, 11);
    std::fs::write(f.outgoing.join("payload.bin"), &body).unwrap();
    let sender = connect(&f.sender_home, &f.outgoing).await;
    let receiver = connect(&f.receiver_home, &f.incoming).await;

    let (sender_job, ticket) = send(
        &sender,
        json!({ "paths": [at(&f.outgoing, "payload.bin")] }),
    )
    .await;
    assert!(ticket.starts_with("sluice1"));
    let started = call(
        &receiver,
        "receive",
        json!({ "ticket": ticket, "out": at(&f.incoming, "landing") }),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    let job = started["result"]["job_id"].as_str().unwrap();
    let status = until(&receiver, job, ended).await;
    assert_eq!(status["state"], "done", "{status}");
    assert_eq!(status["bytes_done"], body.len());
    assert_eq!(
        std::fs::read(f.incoming.join("landing").join("payload.bin")).unwrap(),
        body
    );
    assert_eq!(until(&sender, &sender_job, ended).await["state"], "done");

    // The jobs are listed, and the ticket is in no list.
    let listed = call(&receiver, "list_jobs", json!({})).await;
    assert_eq!(listed["result"]["jobs"].as_array().unwrap().len(), 1);
    assert!(!listed.to_string().contains(&ticket));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_send_returns_its_job_before_the_copy_finishes() {
    let f = folders();
    std::fs::write(f.outgoing.join("large.bin"), data(200 * 1024 * 1024, 2)).unwrap();
    let sender = connect(&f.sender_home, &f.outgoing).await;
    let started = call(
        &sender,
        "send",
        json!({ "paths": [at(&f.outgoing, "large.bin")] }),
    )
    .await;
    let job = started["result"]["job_id"].as_str().unwrap();
    assert!(
        started["result"].get("secret_ticket").is_none(),
        "the ticket is not in the answer"
    );
    let first = call(&sender, "job_status", json!({ "job_id": job })).await;
    assert_eq!(first["result"]["state"], "running");
    assert!(
        first["result"]["secret_ticket"].is_null(),
        "not prepared yet: {first}"
    );
    let later = until(&sender, job, |s| !s["secret_ticket"].is_null()).await;
    assert!(
        later["secret_ticket"]
            .as_str()
            .unwrap()
            .starts_with("sluice1")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_is_marked_as_an_error_and_a_success_is_not() {
    let f = folders();
    let client = connect(&f.sender_home, &f.outgoing).await;
    let refused = call_whole(
        &client,
        "send",
        json!({ "paths": [{ "root": "/not/a/root", "path": "" }] }),
    )
    .await;
    assert_eq!(refused.is_error, Some(true), "{refused:?}");
    assert_eq!(refused.structured_content.unwrap()["ok"], false);
    let listed = call_whole(&client, "list_jobs", json!({})).await;
    assert_ne!(listed.is_error, Some(true), "{listed:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_receive_outside_the_root_is_refused_with_the_reason_in_an_untrusted_field() {
    let f = folders();
    let outside = f._dir.path().join("elsewhere");
    std::fs::create_dir(&outside).unwrap();
    let sender = connect(&f.sender_home, &f.outgoing).await;
    std::fs::write(f.outgoing.join("a.txt"), b"a").unwrap();
    let (_job, ticket) = send(&sender, json!({ "paths": [at(&f.outgoing, "a.txt")] })).await;

    let receiver = connect(&f.receiver_home, &f.incoming).await;
    let refused = call(
        &receiver,
        "receive",
        json!({ "ticket": ticket, "out": at(&outside, "") }),
    )
    .await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["failure"]["kind"], "refused");
    // The sentence is ours and fixed; the details are marked and carry the reason.
    assert_eq!(
        refused["failure"]["message"],
        "a rule of this server refused the request"
    );
    let detail = refused["failure"]["untrusted_detail"].as_str().unwrap();
    assert!(detail.contains("not a root"), "{detail}");
    assert!(
        !refused.to_string().contains(&ticket),
        "the ticket is not echoed"
    );
    assert!(!outside.join("a.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_file_name_comes_back_unchanged_in_an_untrusted_field_and_nowhere_else() {
    let f = folders();
    let hostile = "; rm -rf ~ #.txt";
    let bundle = f.outgoing.join("bundle");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::write(bundle.join(hostile), b"data").unwrap();
    let sender = connect(&f.sender_home, &f.outgoing).await;
    let receiver = connect(&f.receiver_home, &f.incoming).await;
    let (_job, ticket) = send(&sender, json!({ "paths": [at(&f.outgoing, "bundle")] })).await;

    let seen = call(&receiver, "inspect", json!({ "ticket": ticket })).await;
    assert_eq!(seen["ok"], true, "{seen}");
    let names: Vec<&str> = seen["result"]["untrusted_names"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(names.contains(&"bundle/; rm -rf ~ #.txt"), "{names:?}");
    // Nothing the tools wrote outside that field holds the name.
    let mut outside = seen.clone();
    outside["result"]["untrusted_names"] = json!([]);
    assert!(!outside.to_string().contains("rm -rf"), "{outside}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_can_reorder_what_a_terminal_shows_is_refused_and_not_echoed_raw() {
    let f = folders();
    let tricky = f.outgoing.join("bundle2");
    std::fs::create_dir(&tricky).unwrap();
    std::fs::write(tricky.join("report\u{202e}fdp.exe"), b"x").unwrap();
    let sender = connect(&f.sender_home, &f.outgoing).await;
    let started = call(
        &sender,
        "send",
        json!({ "paths": [at(&f.outgoing, "bundle2")] }),
    )
    .await;
    let job = started["result"]["job_id"].as_str().unwrap();
    let status = until(&sender, job, ended).await;
    assert_eq!(status["state"], "failed", "{status}");
    assert!(
        !status.to_string().contains('\u{202e}'),
        "the escape is not passed on raw"
    );
    assert!(
        status["untrusted_detail"].is_string(),
        "the failure is in the marked field: {status}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_receive_leaves_a_store_a_second_receive_resumes() {
    let f = folders();
    let body = data(160 * 1024 * 1024, 5);
    std::fs::write(f.outgoing.join("big.bin"), &body).unwrap();
    let sender = connect(&f.sender_home, &f.outgoing).await;
    let (_job, ticket) = send(
        &sender,
        json!({ "paths": [at(&f.outgoing, "big.bin")], "keep": true }),
    )
    .await;
    let receiver = connect(&f.receiver_home, &f.incoming).await;
    let args = json!({ "ticket": ticket, "out": at(&f.incoming, "big") });
    let first = call(&receiver, "receive", args.clone()).await;
    let job = first["result"]["job_id"].as_str().unwrap().to_string();
    until(&receiver, &job, |s| {
        s["bytes_done"].as_u64().unwrap_or(0) > 0 || ended(s)
    })
    .await;
    let cancelled = call(&receiver, "cancel_job", json!({ "job_id": job })).await;
    let was_running = cancelled["result"]["was_running"].as_bool().unwrap();
    let status = until(&receiver, &job, ended).await;
    // Either the cancel arrived while the job ran, and it ended cancelled with a store kept, or the
    // job had already finished, and the cancel found nothing to stop. The two never mix.
    assert_eq!(
        was_running,
        status["state"] == "cancelled",
        "{cancelled} {status}"
    );
    if was_running {
        assert_eq!(status["resumable"], true);
        let kept = std::fs::read_dir(f.incoming.join("big"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".sluice-partial-")
            });
        assert!(kept, "the partial store stays");
        let second = call(&receiver, "receive", args).await;
        let job = second["result"]["job_id"].as_str().unwrap();
        assert_eq!(until(&receiver, job, ended).await["state"], "done");
    }
    assert_eq!(
        std::fs::read(f.incoming.join("big").join("big.bin")).unwrap(),
        body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_job_is_not_found_and_the_doctor_answers_and_the_client_can_close() {
    let f = folders();
    let client = connect(&f.sender_home, &f.outgoing).await;
    let missing = call(&client, "job_status", json!({ "job_id": "0".repeat(32) })).await;
    assert_eq!(missing["failure"]["kind"], "not_found");
    let doctor = call(&client, "doctor", json!({})).await;
    assert_eq!(doctor["ok"], true);
    client.cancel().await.expect("the client closes its end");
}
