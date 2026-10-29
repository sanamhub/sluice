//! A send that serves several receivers: who may take the content, how many places it offers, and
//! which connections hold one.

use super::*;
use n0_future::StreamExt;

/// A client endpoint and a store that fetch by hand, so one endpoint can connect twice and be the
/// same receiver both times.
pub(super) async fn client(dir: &TempDir, name: &str) -> (Endpoint, FsStore) {
    let endpoint = endpoint::bind(None, &RelayModeChoice::Disabled, Congestion::Cubic, None)
        .await
        .unwrap();
    let store = FsStore::load(dir.path().join(format!("client-store-{name}")))
        .await
        .unwrap();
    (endpoint, store)
}

/// One receiver's whole download over `probe`, which says the transfer is complete when it is. The
/// connection is waited for after the close, so a second dial is a second connection rather than
/// the one that is still closing.
pub(super) async fn take_whole(
    probe: &Endpoint,
    store: &FsStore,
    addr: &iroh::EndpointAddr,
    root: Hash,
) {
    let conn = probe.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        crate::engine::fetch::fetch_whole(store, &conn, root),
    )
    .await
    .expect("the fetch did not finish")
    .unwrap();
    conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");
}

/// Two receivers each take the whole ticket, and the send stops. The sender reports one place and
/// one finish for each, with the same id on a receiver's two lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_receivers_end_a_send_that_serves_that_many() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![5u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move { outgoing.serve(Keep::Receivers(2), &tx).await });

    for name in ["one", "two"] {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        super::receive(&text, &options(&dir.path().join(name)), &tx)
            .await
            .unwrap();
    }
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the send did not stop after two receivers")
        .unwrap()
        .unwrap();
    assert_eq!(end, StopEnd::Downloaded);
    assert_report_lines(&mut events, 2);
}

/// The bound is on endpoints, not on connections: the second place is gone while its holder is
/// still transferring, and a third endpoint that asks for the content is refused. The send ends
/// when the last holder closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_receiver_is_refused_while_a_place_holder_transfers() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![7u8; 4 << 20];
    let (outgoing, text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move {
        outgoing
            .provider
            .serve(
                Stop::Keep {
                    limit: Some(2),
                    roots,
                },
                &tx,
            )
            .await
    });

    // The first place, taken by an ordinary receive.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    super::receive(&text, &options(&dir.path().join("one")), &tx)
        .await
        .unwrap();
    wait_for_started(&mut events).await;

    // The second place, held by a request that is still moving: the stream is opened but only its
    // first update is read, so the connection stays up with bytes still to come.
    let (second, store) = client(&dir, "second").await;
    let held = second
        .connect(addr.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    let request = store
        .remote()
        .execute_get(held.clone(), GetRequest::blob(root));
    let mut stream = std::pin::pin!(request.stream());
    assert!(stream.next().await.is_some(), "the request was not started");
    wait_for_started(&mut events).await;

    // The third endpoint asks for the same blob and is refused before any byte is written.
    refuse_third(&dir, &addr, root, &mut events, 2).await;

    // The holder finishes and closes, which ends the send: both places were taken and neither
    // holder has a connection left.
    finish_held(stream).await;
    held.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");

    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the send did not stop after the last holder closed")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    second.close().await;
}

/// A third endpoint asks for `root` and is refused, and the sender says so once with the limit.
async fn refuse_third(
    dir: &TempDir,
    addr: &iroh::EndpointAddr,
    root: Hash,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    places: u32,
) {
    let (third, store) = client(dir, "third").await;
    let connection = third.connect(addr.clone(), iroh_blobs::ALPN).await.unwrap();
    let refused = store
        .remote()
        .execute_get(connection.clone(), GetRequest::blob(root))
        .complete()
        .await;
    assert!(refused.is_err(), "a third receiver was served");
    wait_for_refused(events, places).await;
    connection.close(0u32.into(), b"refused");
    third.close().await;
}

/// Reads a held request to its end, so the connection can then close with the completion code.
async fn finish_held<S>(mut stream: std::pin::Pin<&mut S>)
where
    S: n0_future::Stream<Item = iroh_blobs::api::remote::GetProgressItem>,
{
    while let Some(item) = stream.next().await {
        match item {
            iroh_blobs::api::remote::GetProgressItem::Done(_) => return,
            iroh_blobs::api::remote::GetProgressItem::Progress(_) => {}
            iroh_blobs::api::remote::GetProgressItem::Error(e) => {
                panic!("the held transfer failed: {e}")
            }
        }
    }
    panic!("the held transfer did not finish");
}

/// Reading only the manifest spends a place: `inspect` against a send that offers one ends it when
/// its connection closes, because a place is about the endpoint, not about the bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inspect_spends_a_place() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![4u8; 64 << 10];
    let (outgoing, text) = prepared(&dir, &body).await;
    let roots = outgoing.roots.clone();
    let (tx, _events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move {
        outgoing
            .provider
            .serve(
                Stop::Keep {
                    limit: Some(1),
                    roots,
                },
                &tx,
            )
            .await
    });

    crate::engine::inspect(&text, Limits::default(), None)
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("an inspect did not spend a place")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
}

/// A receiver that reconnects is one receiver: two connections from one endpoint take one place,
/// and a second endpoint's one connection takes the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_that_reconnects_is_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![6u8; 64 << 10];
    let (outgoing, _text) = prepared(&dir, &body).await;
    let root = Hash::from(outgoing.roots[0]);
    let roots = outgoing.roots.clone();
    let addr = outgoing.provider.addr();
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let serving = tokio::spawn(async move {
        outgoing
            .provider
            .serve(
                Stop::Keep {
                    limit: Some(2),
                    roots,
                },
                &tx,
            )
            .await
    });

    let (probe, store) = client(&dir, "probe").await;
    take_whole(&probe, &store, &addr, root).await;
    assert_lines(&lines_until_finish(&mut events).await, 1);
    // The same endpoint again: a finish line, and no second place line, because it already holds
    // the place it takes.
    take_whole(&probe, &store, &addr, root).await;
    assert_lines(&lines_until_finish(&mut events).await, 0);

    // If the reconnect had taken the second place, the send would have ended when that connection
    // closed and this dial would find nothing to serve.
    let (second, store) = client(&dir, "second").await;
    take_whole(&second, &store, &addr, root).await;
    assert_lines(&lines_until_finish(&mut events).await, 1);
    let end = tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the send did not stop after the second receiver")
        .unwrap()
        .unwrap();
    assert_eq!(end, ServeEnd::Downloaded);
    probe.close().await;
    second.close().await;
}

/// Waits for one receiver's finish line and returns the lines before it, its place line included.
async fn lines_until_finish(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> Vec<Event> {
    let mut lines = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("no receiver event arrived")
            .expect("the events channel closed");
        let finished = matches!(event, Event::ReceiverFinished { .. });
        lines.push(event);
        if finished {
            return lines;
        }
    }
}

/// Checks the lines one connection made: exactly `places` place lines and one finish, in that order.
fn assert_lines(lines: &[Event], places: usize) {
    let started = lines
        .iter()
        .filter(|event| matches!(event, Event::ReceiverStarted { .. }))
        .count();
    assert_eq!(started, places, "the place lines of one connection");
    assert!(
        matches!(lines.last(), Some(Event::ReceiverFinished { .. })),
        "the last line is the finish"
    );
}

/// Drains the sender's receiver lines and checks them: one place and one finish per receiver, the
/// same id on a receiver's two lines, and count receivers in all.
fn assert_report_lines(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, count: usize) {
    let mut started = 0;
    let mut finished = 0;
    let mut ids = BTreeSet::new();
    let mut closed = BTreeSet::new();
    while let Ok(event) = events.try_recv() {
        match event {
            Event::ReceiverStarted { endpoint, relayed } => {
                assert!(!relayed, "a loopback connection was reported relayed");
                started += 1;
                ids.insert(endpoint);
            }
            Event::ReceiverFinished {
                endpoint,
                relayed,
                whole,
            } => {
                assert!(!relayed, "a loopback connection was reported relayed");
                assert!(
                    whole,
                    "a receiver that took everything was reported as part"
                );
                finished += 1;
                closed.insert(endpoint);
            }
            _ => {}
        }
    }
    assert_eq!(
        (started, finished),
        (count, count),
        "one line per receiver and end"
    );
    assert_eq!(ids, closed, "a receiver's two lines name different ids");
    assert_eq!(ids.len(), count, "two receivers shared one id");
}

/// Waits for one receiver's place line, so a test never sleeps between two connections.
async fn wait_for_started(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>) {
    wait_for(events, |event| {
        matches!(event, Event::ReceiverStarted { .. })
    })
    .await;
}

/// Waits for the one refusal line a send makes for an endpoint.
async fn wait_for_refused(events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, places: u32) {
    let event = wait_for(events, |event| {
        matches!(event, Event::ReceiverRefused { .. })
    })
    .await;
    let Event::ReceiverRefused {
        places: reported, ..
    } = event
    else {
        unreachable!("the predicate matched a refusal");
    };
    assert_eq!(reported, places, "the refusal named another limit");
}

async fn wait_for(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    wanted: impl Fn(&Event) -> bool,
) -> Event {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("no receiver event arrived")
            .expect("the events channel closed");
        if wanted(&event) {
            return event;
        }
    }
}
