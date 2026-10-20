//! BLAKE3 hashing for the browser throughput page. Not wired into any product crate: `sluice-wasm`
//! comes later and reuses what the page measures.
//!
//! Two exports, because the engine pays two different costs. [`Pattern::hash`] hashes bytes that are
//! already in WASM memory, which is the hash rate alone. [`Hasher::update`], fed from JavaScript,
//! also pays the copy into WASM memory that every chunk read from a file pays.

mod pattern;

use wasm_bindgen::prelude::*;

/// An incremental BLAKE3 hasher that takes chunks from JavaScript.
///
/// ```
/// let mut hasher = sluice_wasm_hash::Hasher::new();
/// hasher.update(b"abc");
/// assert_eq!(hasher.finalize_hex().len(), 64);
/// ```
#[wasm_bindgen]
pub struct Hasher {
    inner: blake3::Hasher,
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Hasher {
    /// Starts an empty hash.
    ///
    /// ```
    /// let hasher = sluice_wasm_hash::Hasher::new();
    /// assert_eq!(
    ///     hasher.finalize_hex(),
    ///     "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    /// );
    /// ```
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Hasher {
        Hasher {
            inner: blake3::Hasher::new(),
        }
    }

    /// Adds a chunk. From JavaScript the bytes are copied into WASM memory first.
    ///
    /// ```
    /// let mut hasher = sluice_wasm_hash::Hasher::new();
    /// hasher.update(b"a");
    /// hasher.update(b"bc");
    /// let mut whole = sluice_wasm_hash::Hasher::new();
    /// whole.update(b"abc");
    /// assert_eq!(hasher.finalize_hex(), whole.finalize_hex());
    /// ```
    pub fn update(&mut self, chunk: &[u8]) {
        self.inner.update(chunk);
    }

    /// Returns the 32-byte root as 64 hex characters. The hasher stays usable.
    ///
    /// ```
    /// let mut hasher = sluice_wasm_hash::Hasher::new();
    /// hasher.update(&[0]);
    /// assert_eq!(hasher.finalize_hex().len(), 64);
    /// ```
    #[must_use]
    pub fn finalize_hex(&self) -> String {
        self.inner.finalize().to_hex().to_string()
    }
}

/// The BLAKE3 test-vector input (byte `i` is `i % 251`), held in WASM memory.
///
/// ```
/// let pattern = sluice_wasm_hash::Pattern::new(1).unwrap();
/// assert_eq!(
///     pattern.hash(1024).unwrap(),
///     "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"
/// );
/// ```
#[wasm_bindgen]
pub struct Pattern {
    bytes: Vec<u8>,
}

#[wasm_bindgen]
impl Pattern {
    /// Allocates and fills `len` bytes. Throws if the memory cannot be had, so the page can retry
    /// with less. The page times [`Pattern::hash`] and never this.
    ///
    /// ```
    /// assert!(sluice_wasm_hash::Pattern::new(1024).is_ok());
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn new(len: u32) -> Result<Pattern, JsError> {
        let bytes = pattern::alloc_pattern(len as usize)
            .map_err(|_| JsError::new(&format!("cannot allocate {len} bytes in WASM memory")))?;
        Ok(Pattern { bytes })
    }

    /// Hashes all of the bytes in pieces of `chunk_len` and returns the root as hex. Throws if
    /// `chunk_len` is 0.
    ///
    /// ```
    /// let pattern = sluice_wasm_hash::Pattern::new(1025).unwrap();
    /// assert_eq!(
    ///     pattern.hash(1000).unwrap(),
    ///     "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444"
    /// );
    /// ```
    pub fn hash(&self, chunk_len: u32) -> Result<String, JsError> {
        pattern::hash_in_chunks(&self.bytes, chunk_len as usize).map_err(JsError::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hasher_fed_in_odd_chunks_matches_the_pattern_root() {
        for len in [0u32, 1, 1024, 1025, 102_400] {
            let pattern = Pattern::new(len).unwrap();
            let expected = pattern.hash(1 << 20).unwrap();
            let bytes = pattern::alloc_pattern(len as usize).unwrap();
            for chunk_len in [1usize, 999, 4097] {
                let mut hasher = Hasher::new();
                for chunk in bytes.chunks(chunk_len) {
                    hasher.update(chunk);
                }
                assert_eq!(
                    hasher.finalize_hex(),
                    expected,
                    "{len} bytes in chunks of {chunk_len}"
                );
            }
        }
    }
}
