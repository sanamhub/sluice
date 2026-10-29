//! What a send does when the endpoint stops reporting its receivers. No path drops that channel
//! today, but a wait that selected on a closed one would poll it at once for ever; each wait must
//! end with the error that says why.

use super::*;

/// A provider whose receiver reports have ended, and the content roots of its send. The temporary
/// folder is returned so the home stays alive while the provider runs.
async fn closed_reports() -> (TempDir, Provider, Vec<[u8; 32]>) {
    let dir = tempfile::tempdir().unwrap();
    let body = vec![1u8; 1024];
    let (outgoing, _text) = prepared(&dir, &body).await;
    let roots = outgoing.roots.clone();
    let mut provider = outgoing.provider;
    provider.stop_reports();
    (dir, provider, roots)
}

/// A send that waits for Ctrl+C or the expiry is one of the waits that reads the reports; with the
/// channel closed it ends with the error at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keep_wait_ends_when_the_reports_stop() {
    let (dir, provider, roots) = closed_reports().await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let served = tokio::time::timeout(
        Duration::from_secs(2),
        provider.serve(Stop::Keep { limit: None, roots }, &tx),
    )
    .await
    .expect("the wait polled a closed channel instead of ending");
    let err = served.unwrap_err();
    assert!(err.to_string().contains("stopped reporting"), "{err}");
    drop(dir);
}

/// A one-download send reads the same reports to know when its receiver finished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_one_download_wait_ends_when_the_reports_stop() {
    let (dir, provider, roots) = closed_reports().await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let served = tokio::time::timeout(
        Duration::from_secs(2),
        provider.serve(Stop::OneDownload(roots), &tx),
    )
    .await
    .expect("the wait polled a closed channel instead of ending");
    let err = served.unwrap_err();
    assert!(err.to_string().contains("stopped reporting"), "{err}");
    drop(dir);
}
