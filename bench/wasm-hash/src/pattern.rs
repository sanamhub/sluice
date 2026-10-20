//! The BLAKE3 test-vector input and the chunked hashing the page measures. Plain Rust, so it runs
//! under `cargo nextest run` without a browser.

use std::collections::TryReserveError;

/// Length of the repeating test-vector input. The official vectors use byte `i % 251`.
pub const PERIOD: usize = 251;

const BLOCK: [u8; PERIOD] = {
    let mut block = [0u8; PERIOD];
    let mut i = 0;
    while i < PERIOD {
        block[i] = i as u8;
        i += 1;
    }
    block
};

/// Allocates `len` bytes of the test-vector input, or reports that the allocation failed.
///
/// A failed allocation has to be an error and not an abort: the page retries a smaller size on
/// phones, where a 1 GiB buffer is not a given.
pub fn alloc_pattern(len: usize) -> Result<Vec<u8>, TryReserveError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len)?;
    bytes.resize(len, 0);
    // Whole blocks copy in one memcpy each, which beats a modulo per byte on a 1 GiB buffer.
    for chunk in bytes.chunks_mut(PERIOD) {
        chunk.copy_from_slice(&BLOCK[..chunk.len()]);
    }
    Ok(bytes)
}

/// Hashes `data` in pieces of `chunk_len` bytes and returns the root as hex.
///
/// Fails on a zero chunk length, which would otherwise be an endless loop of empty updates.
pub fn hash_in_chunks(data: &[u8], chunk_len: usize) -> Result<String, &'static str> {
    if chunk_len == 0 {
        return Err("chunk length must be at least 1");
    }
    let mut hasher = blake3::Hasher::new();
    for chunk in data.chunks(chunk_len) {
        hasher.update(chunk);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // From test_vectors/test_vectors.json in github.com/BLAKE3-team/BLAKE3, first 32 bytes of `hash`.
    const OFFICIAL: [(usize, &str); 5] = [
        (
            0,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        ),
        (
            1,
            "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
        ),
        (
            1024,
            "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
        ),
        (
            1025,
            "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444",
        ),
        (
            102_400,
            "bc3e3d41a1146b069abffad3c0d44860cf664390afce4d9661f7902e7943e085",
        ),
    ];

    #[test]
    fn pattern_bytes_are_i_mod_251() {
        let bytes = alloc_pattern(3 * PERIOD + 17).unwrap();
        for (i, b) in bytes.iter().enumerate() {
            assert_eq!(*b, (i % 251) as u8, "byte {i}");
        }
    }

    #[test]
    fn root_matches_official_vectors() {
        for (len, expected) in OFFICIAL {
            let bytes = alloc_pattern(len).unwrap();
            assert_eq!(
                hash_in_chunks(&bytes, 1 << 20).unwrap(),
                expected,
                "{len} bytes"
            );
        }
    }

    #[test]
    fn chunked_matches_one_shot_for_chunks_that_do_not_divide_the_length() {
        for len in [1025, 102_400, 1 << 20] {
            let bytes = alloc_pattern(len).unwrap();
            let one_shot = blake3::hash(&bytes).to_hex().to_string();
            for chunk_len in [1, 7, 251, 1000, 1024, 4097, 65_537, len + 1] {
                assert_eq!(
                    hash_in_chunks(&bytes, chunk_len).unwrap(),
                    one_shot,
                    "{len} bytes in chunks of {chunk_len}"
                );
            }
        }
    }

    #[test]
    fn zero_chunk_length_is_an_error() {
        assert!(hash_in_chunks(b"abc", 0).is_err());
    }

    #[test]
    fn impossible_allocation_is_an_error() {
        assert!(alloc_pattern(usize::MAX).is_err());
    }
}
