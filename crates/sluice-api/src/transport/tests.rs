use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_stream::StreamExt;

use super::*;

/// A listener over plain numbers: each accept hands out the next scripted result.
fn scripted(script: Vec<io::Result<u32>>, check: Check<u32>) -> Incoming<u32> {
    let script = Arc::new(std::sync::Mutex::new(script.into_iter()));
    accepting(
        move || {
            let next = script.lock().unwrap().next();
            async move {
                match next {
                    Some(result) => result,
                    None => std::future::pending().await,
                }
            }
        },
        check,
    )
}

#[tokio::test]
async fn a_peer_the_check_refuses_gets_no_connection_and_the_check_was_called() {
    let called = Arc::new(AtomicUsize::new(0));
    let seen = called.clone();
    let check: Check<u32> = Arc::new(move |peer| {
        seen.fetch_add(1, Ordering::SeqCst);
        if *peer == 7 {
            Err(ApiError::Refused("not this user".to_string()))
        } else {
            Ok(())
        }
    });
    let mut incoming = scripted(vec![Ok(7), Ok(8)], check);
    let first = incoming.connections.next().await.unwrap().unwrap();
    assert_eq!(first.stream, 8, "the refused peer 7 was skipped");
    assert_eq!(
        called.load(Ordering::SeqCst),
        2,
        "the check ran for each peer"
    );
}

#[tokio::test(start_paused = true)]
async fn a_client_that_left_at_once_is_skipped_with_no_pause() {
    let check: Check<u32> = Arc::new(|_| Ok(()));
    let reset = || Err(io::Error::from(io::ErrorKind::ConnectionReset));
    let mut incoming = scripted(
        vec![reset(), Err(io::ErrorKind::BrokenPipe.into()), Ok(1)],
        check,
    );
    let before = tokio::time::Instant::now();
    let got = incoming.connections.next().await.unwrap().unwrap();
    assert_eq!(got.stream, 1);
    assert_eq!(
        tokio::time::Instant::now(),
        before,
        "no backoff ran between the failed accepts"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failing_listener_waits_before_it_tries_again() {
    let check: Check<u32> = Arc::new(|_| Ok(()));
    let other = Err(io::Error::other("the listener broke"));
    let mut incoming = scripted(vec![other, Ok(2)], check);
    let before = tokio::time::Instant::now();
    incoming.connections.next().await.unwrap().unwrap();
    assert!(tokio::time::Instant::now() - before >= ACCEPT_BACKOFF);
}
