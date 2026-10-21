//! The modification time and the mode of a file, in the form a manifest carries.

use std::fs::Metadata;
use std::time::UNIX_EPOCH;

/// The modification time in UTC milliseconds. A time before 1970, or one the filesystem does not
/// have, is 0, and one past what a `u64` holds is the largest value, so the cast cannot wrap.
pub(super) fn mtime_ms(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The permission bits. The file type bits that `st_mode` also holds are cut off: they are the
/// entry's `kind`, and a manifest that carried them would be rejected.
#[cfg(unix)]
pub(super) fn mode(meta: &Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o7777
}

/// Windows has no mode bits. A folder is `0o755`, a read-only file `0o444` and any other file
/// `0o644`, which is what a receiver on Unix would make of the one bit Windows does have.
#[cfg(not(unix))]
pub(super) fn mode(meta: &Metadata) -> u32 {
    if meta.is_dir() {
        0o755
    } else if meta.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}
