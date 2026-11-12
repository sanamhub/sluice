//! The listener: a Unix socket inside a private folder of the home, or on Windows a named pipe
//! whose access list names only this user. Never a TCP port, not even on loopback. Every accepted
//! connection has its peer checked before the server sees it.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::server::Connected;

#[cfg(unix)]
pub(crate) mod unix;
#[cfg(unix)]
pub(crate) use unix as platform;

#[cfg(windows)]
pub(crate) mod windows;
#[cfg(windows)]
pub(crate) use windows as platform;

/// How many connections a daemon has open at once. A client that is not served waits in the
/// listener's queue, and one that is gone frees its place.
pub const MAX_CONNECTIONS: usize = 32;

/// How long the accept loop waits after the listener fails, so a broken listener does not spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// A connection that holds one of the daemon's places until it drops.
pub(crate) struct Guarded<S> {
    stream: S,
    _place: OwnedSemaphorePermit,
}

impl<S: AsyncRead + Unpin> AsyncRead for Guarded<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Guarded<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, data)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl<S> Connected for Guarded<S> {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

/// What the server reads connections from, and the task that fills it.
pub(crate) struct Incoming<S> {
    pub(crate) connections: ReceiverStream<io::Result<Guarded<S>>>,
    pub(crate) _task: Accepting,
}

/// The accept task, which stops when this drops.
pub(crate) struct Accepting(JoinHandle<()>);

impl Drop for Accepting {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Accepts connections with `accept`, which returns `None` for a peer it refused, and hands the
/// others on. One place of [`MAX_CONNECTIONS`] is taken before each accept, so the daemon never has
/// more connections open than that.
pub(crate) fn accepting<S, F, Fut>(mut accept: F) -> Incoming<S>
where
    S: Send + 'static,
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = io::Result<Option<S>>> + Send,
{
    let places = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let (tx, rx) = mpsc::channel(1);
    let task = tokio::spawn(async move {
        loop {
            let Ok(place) = places.clone().acquire_owned().await else {
                return;
            };
            match accept().await {
                Ok(Some(stream)) => {
                    let connection = Guarded {
                        stream,
                        _place: place,
                    };
                    if tx.send(Ok(connection)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!("the control listener failed: {error}");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            }
        }
    });
    Incoming {
        connections: ReceiverStream::new(rx),
        _task: Accepting(task),
    }
}
