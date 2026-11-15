//! `sluice daemon --ui` as a process: the calls the page makes, from a send to a verified receive.
//! Everything is on 127.0.0.1.

use crate::common::page::{authorized, get, json_of, page, post, watch};
use serde_json::{Value, json};
use std::time::Duration;

#[test]
fn the_calls_the_page_makes_send_inspect_receive_and_watch_to_done() {
    let page = page();
    let call = |path: &str, body: Value| {
        post(
            &page,
            &format!("/api/v1{path}"),
            "application/json",
            &body.to_string(),
            &[],
        )
    };

    let (a, b) = authorized(&page);
    let version = json_of(
        &get(&page, "/api/v1/version", &[(a.as_str(), b.as_str())]),
        200,
    );
    assert_eq!(version["protocol"], 1);
    let roots = json_of(
        &get(&page, "/api/v1/roots", &[(a.as_str(), b.as_str())]),
        200,
    );
    assert_eq!(roots["roots"], json!([{ "name": "files" }]));
    assert!(
        !roots.to_string().contains(page.root.to_str().unwrap()),
        "no path in the roots"
    );
    let listed = json_of(
        &call("/entries", json!({ "root": "files", "path": "" })),
        200,
    );
    let names: Vec<(&str, &str)> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(names, [("docs", "folder"), ("payload.bin", "file")]);

    // The events are opened before the transfer, as the page does.
    let events = watch(&page);

    let sent = json_of(
        &call(
            "/send",
            json!({ "paths": [{ "root": "files", "path": "payload.bin" }] }),
        ),
        200,
    );
    let ticket = sent["ticket"]["text"].as_str().unwrap().to_string();
    assert!(ticket.starts_with("sluice1"));
    assert!(sent["qr_svg"].as_str().unwrap().starts_with("<svg"));
    assert_eq!(sent["files"], 1);

    let seen = json_of(
        &call("/inspect", json!({ "ticket": { "text": ticket } })),
        200,
    );
    assert_eq!(seen["files"], 1);
    assert!(
        seen["names"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["text"] == "payload.bin")
    );

    let started = json_of(
        &call(
            "/receive",
            json!({ "ticket": { "text": ticket }, "out": { "root": "files", "path": "landing" } }),
        ),
        200,
    );
    let job = started["job_id"].as_str().unwrap().to_string();

    // The receive's events through `done`, one object per line.
    let mut verified = false;
    let state = loop {
        let event = events
            .recv_timeout(Duration::from_secs(120))
            .expect("the events keep coming");
        if event["job_id"] != job.as_str() {
            continue;
        }
        verified |= event.get("verified").is_some();
        if let Some(done) = event.get("done") {
            break done["state"].as_str().unwrap().to_string();
        }
    };
    assert_eq!(state, "done");
    assert!(verified, "a verified event came before done");
    let want = crate::common::data(300 * 1024, 5);
    assert_eq!(
        std::fs::read(page.root.join("landing").join("payload.bin")).unwrap(),
        want
    );

    let jobs = json_of(
        &get(&page, "/api/v1/jobs", &[(a.as_str(), b.as_str())]),
        200,
    );
    assert!(
        jobs["jobs"].as_array().unwrap().len() >= 2,
        "the send and the receive are listed"
    );
    let cancelled = call(&format!("/jobs/{job}/cancel"), json!({}));
    assert_eq!(json_of(&cancelled, 200)["was_running"], false);
}

#[test]
fn an_error_is_a_json_object_with_a_sentence_and_a_status() {
    let page = page();
    let answer = post(
        &page,
        "/api/v1/inspect",
        "application/json",
        r#"{"ticket":{"text":"nonsense"}}"#,
        &[],
    );
    assert!((400..600).contains(&answer.status), "{}", answer.status);
    let body: Value = serde_json::from_str(&answer.body).unwrap();
    assert!(body["error"].is_string(), "{body}");
    assert!(
        !answer.body.contains("nonsense"),
        "the ticket text is not echoed"
    );
    let missing = post(
        &page,
        "/api/v1/entries",
        "application/json",
        r#"{"root":"files"}"#,
        &[],
    );
    assert_eq!(missing.status, 400);
}

#[test]
fn the_port_file_is_private_and_goes_when_the_daemon_stops() {
    let page = page();
    let file = page.daemon.home.join("ui.json");
    assert!(file.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // `sluice ui --print` finds the daemon through it, and says the address holds the key.
    let found = crate::common::sluice(&page.daemon.home)
        .args(["ui", "--print"])
        .assert()
        .success();
    let url = crate::common::stdout_text(&found);
    assert_eq!(
        url.trim(),
        format!("http://127.0.0.1:{}/#key={}", page.port, page.key)
    );
    assert!(crate::common::stderr_text(&found).contains("holds the key"));
    crate::common::sluice(&page.daemon.home)
        .args(["daemon", "stop"])
        .assert()
        .success();
    assert!(!file.exists(), "the file is removed when the daemon stops");
}

#[test]
fn with_no_daemon_ui_says_how_to_start_one() {
    let dir = tempfile::tempdir().unwrap();
    let failed = crate::common::sluice(&dir.path().join("home"))
        .args(["ui", "--print"])
        .assert()
        .failure();
    let text = crate::common::stderr_text(&failed);
    assert!(text.contains("sluice daemon --ui"), "{text}");
    assert!(!text.contains("key="), "no key is printed");
}
