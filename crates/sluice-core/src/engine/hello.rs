//! ALPN `sluice/1` and its `Hello`, as `SPEC.md` section 3 states it. Static content has its own
//! ALPN and connection, so this handler never carries a blob.

use iroh::{
    endpoint::{Connection, RecvStream, SendStream, VarInt},
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::io::AsyncWriteExt;

/// The ALPN of every protocol in `SPEC.md` other than static content.
pub const ALPN: &[u8] = b"sluice/1";

/// The only `proto_version` this release speaks.
pub const PROTO_VERSION: u16 = 1;

/// A live stream is understood, bit 0.
pub const CAP_LIVE: u64 = 1 << 0;
/// Delta hints are understood, bit 1.
pub const CAP_DELTA: u64 = 1 << 1;
/// Small-file packs are understood, bit 2.
pub const CAP_PACKS: u64 = 1 << 2;
/// Manifests of version 1 are understood, bit 3.
pub const CAP_MANIFEST_V1: u64 = 1 << 3;
/// Receipts are understood, bit 4.
pub const CAP_RECEIPTS: u64 = 1 << 4;
/// zstd content encoding is understood, bit 5.
pub const CAP_ZSTD: u64 = 1 << 5;

/// The capabilities this release offers. A capability is usable only when both sides set it.
pub const CAPABILITIES: u64 = CAP_PACKS | CAP_MANIFEST_V1;
/// The largest frame this side accepts. The sender must not exceed it.
pub const MAX_FRAME_BYTES: u32 = 16 << 20;

/// The first record on a `sluice/1` control stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The protocol version, 1 here.
    pub proto_version: u16,
    /// The capability bitset.
    pub capabilities: u64,
    /// The receiver's frame limit in bytes.
    pub max_frame_bytes: u32,
}

impl Hello {
    /// This release's greeting.
    pub const fn ours() -> Self {
        Self {
            proto_version: PROTO_VERSION,
            capabilities: CAPABILITIES,
            max_frame_bytes: MAX_FRAME_BYTES,
        }
    }

    /// Whether both sides offered `capability`.
    pub fn both(&self, other: &Self, capability: u64) -> bool {
        self.capabilities & other.capabilities & capability != 0
    }
}

/// The greeting's bytes: the three shortest postcard varints, with nothing before or after them.
pub fn encode(hello: Hello) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    for value in [
        u64::from(hello.proto_version),
        hello.capabilities,
        u64::from(hello.max_frame_bytes),
    ] {
        let mut value = value;
        loop {
            let low = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(low);
                break;
            }
            out.push(low | 0x80);
        }
    }
    out
}

/// Reads the greeting from its bytes and refuses another protocol version, a frame limit of zero,
/// trailing bytes, a varint longer than needed and a varint that overflows.
#[allow(dead_code)] // The greeting bytes are checked here and by the P3 vectors.
pub fn decode(bytes: &[u8]) -> Result<Hello, std::io::Error> {
    let bad = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string());
    let mut at = 0;
    let version = take_varint(bytes, &mut at).ok_or_else(|| bad("Hello ends early"))?;
    let capabilities = take_varint(bytes, &mut at).ok_or_else(|| bad("Hello ends early"))?;
    let limit = take_varint(bytes, &mut at).ok_or_else(|| bad("Hello ends early"))?;
    if at != bytes.len() {
        return Err(bad("bytes follow the Hello"));
    }
    let proto_version = u16::try_from(version).map_err(|_| bad("proto_version overflows"))?;
    if proto_version != PROTO_VERSION {
        return Err(bad("proto_version is not 1"));
    }
    let max_frame_bytes = u32::try_from(limit).map_err(|_| bad("frame limit overflows"))?;
    if max_frame_bytes == 0 {
        return Err(bad("frame limit is zero"));
    }
    Ok(Hello {
        proto_version,
        capabilities,
        max_frame_bytes,
    })
}

/// Writes the greeting and leaves the stream open for the records that follow it.
pub async fn write_hello(send: &mut SendStream, hello: Hello) -> std::io::Result<()> {
    send.write_all(&encode(hello)).await?;
    send.flush().await
}

/// Reads one greeting one varint at a time, because the three fields say where it ends without a
/// length prefix. A stream that ends before the third varint is an error.
pub async fn read_hello(recv: &mut RecvStream) -> Result<Hello, std::io::Error> {
    let version = read_stream_varint(recv).await?;
    let capabilities = read_stream_varint(recv).await?;
    let limit = read_stream_varint(recv).await?;
    let bad = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string());
    let proto_version = u16::try_from(version).map_err(|_| bad("proto_version overflows"))?;
    if proto_version != PROTO_VERSION {
        return Err(bad("proto_version is not 1"));
    }
    let max_frame_bytes = u32::try_from(limit).map_err(|_| bad("frame limit overflows"))?;
    if max_frame_bytes == 0 {
        return Err(bad("frame limit is zero"));
    }
    Ok(Hello {
        proto_version,
        capabilities,
        max_frame_bytes,
    })
}

async fn read_stream_varint(recv: &mut RecvStream) -> Result<u64, std::io::Error> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let mut byte = [0u8; 1];
        recv.read_exact(&mut byte)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "a Hello varint is longer than u64",
    ))
}

fn take_one(bytes: &[u8], at: &mut usize) -> Result<u8, ()> {
    let byte = *bytes.get(*at).ok_or(())?;
    *at += 1;
    Ok(byte)
}

fn take_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = take_one(bytes, at).ok()?;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The dialer's half of the greeting. It writes `Hello` before it reads the answer, because
/// `open_bi` is lazy: the listener is not told of the stream until the dialer writes to it. A
/// dialer that read first would wait on a stream the listener has not seen.
pub async fn greet(conn: &Connection) -> std::io::Result<Hello> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(std::io::Error::other)?;
    write_hello(&mut send, Hello::ours()).await?;
    let theirs = read_hello(&mut recv).await?;
    send.finish()?;
    Ok(theirs)
}

/// The close code for a `sluice/1` stream that names another protocol version.
const CLOSE_VERSION: u32 = 1;

/// The `sluice/1` listener. It reads the dialer's greeting, rejects another protocol version, and
/// answers with its own. A later change adds the control records after this exchange.
#[derive(Clone, Copy, Debug, Default)]
pub struct SluiceProtocol;

impl ProtocolHandler for SluiceProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = conn.accept_bi().await?;
        let _theirs = match read_hello(&mut recv).await {
            Ok(hello) => hello,
            Err(cause) => {
                // SPEC section 3 asks for a clear reason, which a generic accept error does not
                // carry to the peer.
                conn.close(
                    VarInt::from_u32(CLOSE_VERSION),
                    cause.to_string().as_bytes(),
                );
                return Err(AcceptError::from_err(cause));
            }
        };
        write_hello(&mut send, Hello::ours())
            .await
            .map_err(AcceptError::from_err)?;
        send.finish()?;
        // Returning from `accept` closes the connection, which can discard the greeting before the
        // dialer reads it. The handler owns the connection until the dialer is done with it.
        let _ = conn.closed().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CAP_LIVE, CAP_MANIFEST_V1, Hello, decode, encode};

    #[test]
    fn greeting_round_trips() {
        let hello = Hello::ours();
        assert_eq!(decode(&encode(hello)).unwrap(), hello);
    }

    #[test]
    fn a_capability_needs_both_sides() {
        let ours = Hello::ours();
        let other = Hello {
            capabilities: CAP_LIVE | CAP_MANIFEST_V1,
            ..ours
        };
        assert!(!ours.both(&other, CAP_LIVE));
        assert!(ours.both(&other, CAP_MANIFEST_V1));
    }

    #[test]
    fn another_version_and_trailing_bytes_are_refused() {
        let mut bytes = encode(Hello::ours());
        bytes[0] = 2;
        assert!(decode(&bytes).is_err());
        let mut bytes = encode(Hello::ours());
        bytes.push(0);
        assert!(decode(&bytes).is_err());
    }
}
