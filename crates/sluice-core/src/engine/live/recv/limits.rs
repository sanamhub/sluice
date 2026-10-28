//! The two limits a live receive stops at: the free room the next segment must leave, and the size
//! the file may not pass.

use std::path::{Path, PathBuf};

use crate::{
    Error,
    engine::{ReceiveOptions, storage},
};

/// A live stream that stopped before the close: the error to report, and whether the bytes that
/// arrived are the start of the file. Only the size bound leaves a prefix worth keeping; a broken
/// chain or a failed fetch leaves nothing that can be trusted.
pub(super) struct Stop {
    pub(super) error: Error,
    pub(super) keep: bool,
}

impl Stop {
    pub(super) fn keep(error: Error) -> Self {
        Self { error, keep: true }
    }
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Self { error, keep: false }
    }
}

/// The room a live receive must leave free on the output's volume after the next segment. A live
/// stream has no known end, so nothing else would stop it before the disk is full.
const FREE_FLOOR: u64 = 1 << 30;

/// The two limits a live stream stops at, checked before each segment is fetched: the free room the
/// next segment must leave on every volume it fills, and the size the file may not pass.
pub(super) struct Bounds {
    /// The volume the output file is written on.
    output: PathBuf,
    /// The volume the temporary store of the segments is written on, which is the home's `tmp` and
    /// may be a different disk.
    store: PathBuf,
    max_size: Option<u64>,
    /// Free bytes for the output and for the store, given instead of asking the volumes, for a test
    /// that has no full disk.
    free: Option<(u64, u64)>,
}

impl Bounds {
    pub(super) fn new(options: &ReceiveOptions, store: &Path) -> Self {
        Self {
            output: options
                .out
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            store: store.to_path_buf(),
            max_size: options.max_size,
            free: options.free_bytes.map(|bytes| (bytes, bytes)),
        }
    }

    #[cfg(test)]
    pub(super) fn with_free(
        output: &Path,
        store: &Path,
        max_size: Option<u64>,
        free: (u64, u64),
    ) -> Self {
        Self {
            output: output
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            store: store.to_path_buf(),
            max_size,
            free: Some(free),
        }
    }

    /// Refuses a segment that would pass the size or leave too little room on either volume. The
    /// store holds every segment until it is written out, so a stream can fill the disk the home is
    /// on while the output's disk still has room.
    pub(super) fn check(&self, total: u64, len: u32) -> Result<(), Stop> {
        let len = u64::from(len);
        if let Some(max) = self.max_size
            && total.saturating_add(len) > max
        {
            return Err(Stop::keep(Error::Storage(format!(
                "the file would pass --max-size of {max} bytes; the {total} bytes that arrived are \
                 the start of it"
            ))));
        }
        for (index, volume) in [&self.output, &self.store].into_iter().enumerate() {
            let available = match self.free {
                Some((output, store)) => {
                    if index == 0 {
                        output
                    } else {
                        store
                    }
                }
                None => storage::available_bytes(volume).map_err(Stop::from)?,
            };
            if available.saturating_sub(len) < FREE_FLOOR {
                return Err(Stop::keep(Error::Storage(format!(
                    "{} would have less than {FREE_FLOOR} bytes free after the next segment; the \
                     {total} bytes that arrived are the start of the file",
                    volume.display()
                ))));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, FREE_FLOOR};
    use std::path::Path;

    /// A live receive keeps every segment in the store until it is written out, and the store is
    /// under the home's `tmp`, which may be another volume from the output. The floor belongs to
    /// both: a stream that fills the home's disk cannot be read back either.
    #[test]
    fn the_store_volume_must_keep_the_floor_too() {
        let out = Path::new("out/file.bin");
        let store = Path::new("home/tmp/store");
        let fixed = |free| Bounds::with_free(out, store, None, free);
        let err = fixed((FREE_FLOOR * 4, FREE_FLOOR / 2))
            .check(0, 16 * 1024)
            .unwrap_err();
        assert!(
            err.error.to_string().contains("home/tmp/store"),
            "{}",
            err.error
        );
        assert!(
            fixed((FREE_FLOOR * 2, FREE_FLOOR * 2))
                .check(0, 16 * 1024)
                .is_ok()
        );
        let err = fixed((FREE_FLOOR, FREE_FLOOR * 2))
            .check(0, 16 * 1024)
            .unwrap_err();
        assert!(err.error.to_string().contains("out"), "{}", err.error);
    }
}
