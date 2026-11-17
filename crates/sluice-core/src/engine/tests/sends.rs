//! A send that attaches to the daemon's identity endpoint, and the places it offers there.

use super::*;
use crate::engine::Listener;

/// A send in the daemon's process installs its handlers on the daemon's endpoint, so the ticket
/// names the daemon's id and not a second endpoint's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_attaches_to_the_daemons_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let body = b"attached to the daemon";
    let source = dir.path().join("note.txt");
    std::fs::write(&source, body).unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let listener = Listener::bind(&home, RelayModeChoice::Loopback, None)
        .await
        .unwrap();
    listener.register();

    let mut outgoing = prepare_send(
        &home,
        &source,
        RelayModeChoice::Loopback,
        Congestion::Cubic,
        None,
        None,
    )
    .await
    .unwrap();
    outgoing.reserve(Keep::One);
    let text = outgoing.ticket.encode();
    let named = Ticket::decode(&text).unwrap();
    assert_eq!(named.endpoint_id(), &listener.id());
    let (events, _received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _ = outgoing.serve(Keep::One, &events).await;
    });

    let out = dir.path().join("out");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let inbound = out.clone();
    let receiving =
        tokio::spawn(async move { super::receive(&text, &options(&inbound), &tx).await });
    let received = tokio::time::timeout(Duration::from_secs(30), receiving)
        .await
        .expect("the receive ended")
        .unwrap()
        .unwrap();
    assert_eq!(received.files, 1);
    assert_eq!(std::fs::read(out.join("note.txt")).unwrap(), body);
    listener.stop();
}
