//! A delivery, `SPEC.md` section 11.3: one stream carries a ticket and an optional name, the daemon
//! answers one status byte, and the content arrives on the static transport as an ordinary receive.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{
    endpoint::{Connection, RecvStream},
    protocol::{AcceptError, ProtocolHandler},
};

use super::limits::{Held, lock};
use crate::device::{Devices, Grant};
use crate::engine::{Congestion, ReceiveOptions};
use crate::manifest::Limits;
use crate::queue::MAX_NETWORK_FAILURES;
use crate::ticket::{Body, Ticket};

/// The most a delivery request may hold: a ticket of 4096 bytes, a name of 255, and the framing.
const MAX_REQUEST: usize = 8192;
/// The longest name a delivery may give the top-level entry.
const MAX_NAME: usize = 255;
/// How long the daemon waits for the request to end.
const READ_WAIT: Duration = Duration::from_secs(10);
/// How long the handler waits for the client to close after the status byte.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The folder a daemon receives deliveries into, and how much content it may hold.
#[derive(Clone, Debug)]
pub struct Inbox {
    /// The folder, inside one of the daemon's roots.
    pub path: PathBuf,
    /// The most content the inbox may hold, in bytes. A delivery reserves the size its ticket
    /// states before it starts.
    pub quota: u64,
}

/// What the daemon has accepted and not yet seen end, so two deliveries cannot take the last
/// gigabyte between them.
pub(super) struct Deliverer {
    pub(super) inbox: Inbox,
    devices: Devices,
    reserved: Mutex<u64>,
}

impl Deliverer {
    pub(super) fn new(inbox: Inbox, devices: Devices) -> Self {
        Self {
            inbox,
            devices,
            reserved: Mutex::new(0),
        }
    }

    /// Checks the grant, the ticket and the room, reserves the size, starts the receive, and
    /// answers the status byte. A refusal is decided before any content arrives. The delivery lands
    /// in a top-level entry of the inbox named by `name`, or by the ticket's root when it has none.
    pub(super) fn start(self: &Arc<Self>, peer: [u8; 32], text: &str, name: Option<&str>) -> u8 {
        if !self.devices.allows(&peer, Grant::Deliver) {
            return 0x01;
        }
        let Ok(ticket) = Ticket::decode(text) else {
            return 0x01;
        };
        let (root, size) = match ticket.body() {
            Body::Static { root, size, .. } => (*root, *size),
            // A live stream states no size, and this release does not receive one into an inbox: it
            // would have to stop at the quota and leave a partial file. Refused before any byte,
            // which is the same answer a full inbox gives.
            Body::Live { .. } | Body::Pairing { .. } => return 0x01,
        };
        let Some(placement) = placement(name, &root) else {
            return 0x01;
        };
        // The top-level entry is made here and not by the receive: `create_dir` refuses a name that
        // is already there, so two deliveries cannot share one, and nothing in the inbox is ever
        // overwritten.
        let out = self.inbox.path.join(&placement);
        match std::fs::create_dir(&out) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return 0x01,
            Err(_) => return 0x02,
        }
        let mut reserved = lock(&self.reserved);
        let used = used(&self.inbox.path);
        let room = self
            .inbox
            .quota
            .saturating_sub(used)
            .saturating_sub(*reserved);
        if size > room {
            drop(reserved);
            let _ = std::fs::remove_dir(&out);
            return 0x01;
        }
        *reserved = reserved.saturating_add(size);
        drop(reserved);

        let this = Arc::clone(self);
        let text = text.to_string();
        n0_future::task::spawn(async move {
            let (events, _received) = tokio::sync::mpsc::unbounded_channel();
            let refused =
                crate::engine::receive(&text, &options(&this.inbox.path.join(&placement)), &events)
                    .await
                    .is_err();
            if refused {
                // A receive that failed before it wrote anything leaves no trace, so the name can
                // be used by a later delivery. A partial transfer keeps its folder and its bytes.
                let _ = std::fs::remove_dir(&out);
            }
            let mut reserved = lock(&this.reserved);
            *reserved = reserved.saturating_sub(size);
        });
        0x00
    }
}

/// The one component a delivery lands under: the `name` the request gave, checked by the path rules
/// of SPEC.md section 6, or the ticket's root in lower-case hex. `None` is a name the rules refuse.
fn placement(name: Option<&str>, root: &[u8; 32]) -> Option<String> {
    match name {
        Some(name) => {
            let path = crate::manifest::path::ManifestPath::parse(name).ok()?;
            // A delivery names one entry under the inbox, not a path below one.
            if path.as_str().contains('/') {
                return None;
            }
            Some(path.as_str().to_string())
        }
        None => Some(root.iter().map(|byte| format!("{byte:02x}")).collect()),
    }
}

/// The receive a delivery runs: an ordinary receive into the inbox, under a key made for this one
/// receive, with no receipt, no delta and no overwrite.
fn options(inbox: &Path) -> ReceiveOptions {
    ReceiveOptions {
        out: inbox.to_path_buf(),
        force: false,
        no_times: false,
        links: false,
        allow_hidden: false,
        limits: Limits::default(),
        free_bytes: None,
        max_size: None,
        congestion: Congestion::Cubic,
        rate: None,
        write_limit: None,
        max_network_failures: MAX_NETWORK_FAILURES,
        include: None,
        receipt: None,
        delta_from: None,
        delta_wait: None,
    }
}

/// The bytes the inbox holds now.
fn used(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![(path.to_path_buf(), 0u32)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 32 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let entry_path = entry.path();
            if kind.is_dir() {
                stack.push((entry_path, depth + 1));
            } else if kind.is_file()
                && let Ok(meta) = entry.metadata()
            {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// One delivery listener on `/sluice/deliver/1`.
pub(super) struct DeliverHandler {
    pub(super) deliver: Arc<Deliverer>,
    pub(super) devices: Devices,
    pub(super) held: Held,
}

impl std::fmt::Debug for DeliverHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliverHandler").finish_non_exhaustive()
    }
}

impl ProtocolHandler for DeliverHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = *conn.remote_id().as_bytes();
        let granted = self.devices.allows(&peer, Grant::Deliver);
        let held = self.held.hold(&conn);
        if let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let status = if granted {
                self.answer(&mut recv, peer).await
            } else {
                // A peer with no grant is answered and its request is not read.
                0x01
            };
            let _ = send.write_all(&[status]).await;
            let _ = send.finish();
        }
        let _ = tokio::time::timeout(CLOSE_WAIT, conn.closed()).await;
        drop(held);
        Ok(())
    }
}

impl DeliverHandler {
    async fn answer(&self, recv: &mut RecvStream, peer: [u8; 32]) -> u8 {
        let read = tokio::time::timeout(READ_WAIT, recv.read_to_end(MAX_REQUEST)).await;
        let Ok(Ok(bytes)) = read else {
            return 0x01;
        };
        match request(&bytes) {
            Some((text, name)) => self.deliver.start(peer, &text, name.as_deref()),
            None => 0x01,
        }
    }
}

/// A delivery request: a varint length and that many bytes of ticket text, then `0x00` for no name
/// or `0x01`, a varint length and that many bytes of UTF-8. Anything after it is a refusal.
fn request(bytes: &[u8]) -> Option<(String, Option<String>)> {
    let mut at = 0;
    let length = varint(bytes, &mut at)? as usize;
    if length == 0 || length > 4096 {
        return None;
    }
    let text = std::str::from_utf8(bytes.get(at..at + length)?).ok()?;
    at += length;
    let named = match *bytes.get(at)? {
        0x00 => {
            at += 1;
            None
        }
        0x01 => {
            at += 1;
            let length = varint(bytes, &mut at)? as usize;
            if length > MAX_NAME {
                return None;
            }
            let name = std::str::from_utf8(bytes.get(at..at + length)?).ok()?;
            at += length;
            Some(name.to_string())
        }
        _ => return None,
    };
    (at == bytes.len()).then(|| (text.to_string(), named))
}

/// A LEB128 varint from `bytes` at `at`, which moves past it.
fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *bytes.get(*at)?;
        *at += 1;
        if shift >= 64 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            // The shortest spelling, as SPEC.md section 2 requires.
            if byte == 0 && shift > 0 {
                return None;
            }
            return Some(value);
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::{request, varint};

    #[test]
    fn a_request_is_a_ticket_and_an_optional_name() {
        let mut bytes = vec![3, b'a', b'b', b'c', 0];
        assert_eq!(request(&bytes), Some(("abc".to_string(), None)), "no name");
        bytes = vec![3, b'a', b'b', b'c', 1, 2, b'h', b'i'];
        assert_eq!(
            request(&bytes),
            Some(("abc".to_string(), Some("hi".to_string())))
        );
        // A byte after the name, or a name without its length, is nothing.
        bytes.push(0);
        assert_eq!(request(&bytes), None);
        assert_eq!(request(&[0]), None);
        assert_eq!(request(&[1, b'a', 2]), None);
    }

    #[test]
    fn a_varint_is_the_shortest_spelling() {
        let mut at = 0;
        assert_eq!(varint(&[0x00], &mut at), Some(0));
        at = 0;
        assert_eq!(varint(&[0xe8, 0x07], &mut at), Some(1000));
        at = 0;
        assert_eq!(varint(&[0x80, 0x00], &mut at), None, "a zero byte too many");
    }
}
