//! Raw mode: bytes over QUIC streams with no blobs protocol and no hashing, to separate the cost
//! of QUIC from the cost of verification.

use anyhow::{Context, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};

use crate::data::{RAW_BLOCK_BYTES, generate};

/// Answers each connection's request with that many bytes on one unidirectional stream.
#[derive(Debug)]
pub struct RawHandler {
    block: Vec<u8>,
}

impl RawHandler {
    /// Builds the block that is repeated to fill a stream. QUIC neither compresses nor
    /// deduplicates, so repeating a pseudorandom block costs the same on the wire as fresh data.
    pub fn new(seed: u64, size: u64) -> Result<Self> {
        Ok(Self {
            block: generate(seed, size.min(RAW_BLOCK_BYTES))?,
        })
    }
}

impl ProtocolHandler for RawHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let mut request = conn.accept_uni().await?;
        let mut len = [0u8; 8];
        request
            .read_exact(&mut len)
            .await
            .map_err(AcceptError::from_err)?;
        let mut remaining = u64::from_le_bytes(len);

        let mut send = conn.open_uni().await?;
        let mut pos = 0;
        while remaining > 0 {
            let room = self.block.len() - pos;
            let n = room.min(usize::try_from(remaining).unwrap_or(usize::MAX));
            send.write_all(&self.block[pos..pos + n])
                .await
                .map_err(AcceptError::from_err)?;
            remaining -= n as u64;
            pos = (pos + n) % self.block.len();
        }
        send.finish()?;
        // Dropping the connection now would discard bytes still in flight.
        conn.closed().await;
        Ok(())
    }
}

/// Asks for `len` bytes on this connection and counts what arrives.
pub async fn receive(conn: &Connection, len: u64) -> Result<u64> {
    let mut request = conn.open_uni().await.context("open request stream")?;
    request
        .write_all(&len.to_le_bytes())
        .await
        .context("send request")?;
    request.finish().context("finish request stream")?;

    let mut data = conn.accept_uni().await.context("accept data stream")?;
    let mut total = 0u64;
    while let Some(chunk) = data.read_chunk(usize::MAX).await.context("read data")? {
        total += chunk.len() as u64;
    }
    Ok(total)
}

/// Splits `size` into `parts` lengths that sum to `size`. The last takes the remainder.
pub fn split(size: u64, parts: u16) -> Vec<u64> {
    let parts = u64::from(parts);
    let each = size / parts;
    let mut lens = vec![each; parts as usize];
    if let Some(last) = lens.last_mut() {
        *last += size - each * parts;
    }
    lens
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn lengths_sum_to_the_size() {
        for (size, parts) in [(8 << 20, 4), (10, 3), (1, 1), (1 << 30, 16)] {
            let lens = split(size, parts);
            assert_eq!(lens.len(), parts as usize);
            assert_eq!(lens.iter().sum::<u64>(), size);
        }
    }
}
