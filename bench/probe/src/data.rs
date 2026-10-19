//! Test data derived from a seed, so neither end needs a data file and both can compute the same
//! BLAKE3 root.

use anyhow::{Context, Result};

/// Buffer size when streaming generated data. Large enough to keep the hasher on its SIMD paths.
const STEP: usize = 1 << 20;

/// Raw mode repeats a block this long, so the sender never waits on the generator.
pub const RAW_BLOCK_BYTES: u64 = 64 << 20;

fn reader(seed: u64) -> blake3::OutputReader {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seed.to_le_bytes());
    hasher.finalize_xof()
}

/// The first `len` bytes of the BLAKE3 extendable output of `seed`. It is not compressible.
pub fn generate(seed: u64, len: u64) -> Result<Vec<u8>> {
    let len = usize::try_from(len).context("test data does not fit in this machine's memory")?;
    let mut data = vec![0u8; len];
    reader(seed).fill(&mut data);
    Ok(data)
}

/// The BLAKE3 root of `generate(seed, size)`, computed in constant memory.
pub fn expected_hash(seed: u64, size: u64) -> blake3::Hash {
    let mut xof = reader(seed);
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; STEP];
    let mut left = size;
    while left > 0 {
        let n = usize::try_from(left.min(STEP as u64)).expect("bounded by STEP");
        xof.fill(&mut buf[..n]);
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::{expected_hash, generate};

    #[test]
    fn streamed_hash_matches_the_hash_of_the_whole_buffer() {
        let size = (3 << 20) + 17;
        let data = generate(7, size).unwrap();
        assert_eq!(expected_hash(7, size), blake3::hash(&data));
    }

    #[test]
    fn seeds_give_different_data() {
        assert_ne!(generate(1, 64).unwrap(), generate(2, 64).unwrap());
    }
}
