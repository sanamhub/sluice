//! The record framing of the `log` fuzz target. `cargo xtask fuzz-seeds` includes this same file
//! to write its seeds, so the seeds and the target cannot drift apart.
//!
//! A record is a tag byte, whose low bit says frame (0) or close (1), a length byte, and that many
//! bytes. The last record of an input may be cut short, and then holds what is left.

/// The longest record a length byte can say.
pub const MAX_RECORD_BYTES: usize = 255;

/// The records of an input, as whether each is a close, and its bytes.
pub fn records(mut data: &[u8]) -> impl Iterator<Item = (bool, &[u8])> {
    std::iter::from_fn(move || {
        let (&tag, rest) = data.split_first()?;
        let (&len, rest) = rest.split_first().unwrap_or((&0, &[]));
        let (record, rest) = rest.split_at(usize::from(len).min(rest.len()));
        data = rest;
        Some((tag & 1 == 1, record))
    })
}

/// Adds one record to `input`, or gives back its length if that is over [`MAX_RECORD_BYTES`].
pub fn push(input: &mut Vec<u8>, is_close: bool, bytes: &[u8]) -> Result<(), usize> {
    let len = u8::try_from(bytes.len()).map_err(|_| bytes.len())?;
    input.push(u8::from(is_close));
    input.push(len);
    input.extend_from_slice(bytes);
    Ok(())
}
