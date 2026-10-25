//! Reading an access list back and comparing it with the private one. No unsafe code: the list is
//! read and converted by the parent module, and everything here is text.

use std::io;
use std::path::Path;

use super::{INHERIT, canonical_sddl, dacl_sddl, private_sddl, user_sid};

/// Whether the file at `path` has the private access list [`super::restrict_file`] sets:
/// inheritance stopped, and full control for the user, `SYSTEM` and `Administrators`, with nothing
/// else.
pub(in crate::home) fn is_private_file(path: &Path) -> io::Result<bool> {
    has_private_list(path, "")
}

/// Whether the folder at `path` has the private access list [`super::restrict_dir`] sets on a new
/// home: the same three entries, each with the inherit flags that pass it to what is made inside.
pub(in crate::home) fn is_private_dir(path: &Path) -> io::Result<bool> {
    has_private_list(path, INHERIT)
}

/// Whether the list of `path` is protected and holds exactly the entries the store writes with
/// `inherit`. It is read back by path and compared entry by entry, because Windows may give the
/// entries back in another order.
fn has_private_list(path: &Path, inherit: &str) -> io::Result<bool> {
    has_private_list_text(&dacl_sddl(path)?, inherit)
}

/// [`has_private_list`] over the text of a list that was read back, so a caller with a handle can
/// compare what its own handle returned.
pub(super) fn has_private_list_text(found: &str, inherit: &str) -> io::Result<bool> {
    has_private_list_for(found, inherit, &user_sid()?)
}

/// [`has_private_list_text`] for the user `sid`, so a test can be an account other than the one
/// running it. Both texts go through the conversion Windows uses to write a list back, because
/// that is where a full SID becomes the alias the system reads it as: comparing the text as it was
/// built would refuse a list that is right.
pub(super) fn has_private_list_for(found: &str, inherit: &str, sid: &str) -> io::Result<bool> {
    let wanted = canonical_sddl(&private_sddl(sid, inherit))?;
    let found = canonical_sddl(found)?;
    let (found_protected, found_entries) = split_sddl(&found);
    let (_, wanted_entries) = split_sddl(&wanted);
    Ok(found_protected && found_entries == wanted_entries)
}

/// Whether an SDDL access list is protected from inheritance, and its entries, sorted.
fn split_sddl(sddl: &str) -> (bool, Vec<&str>) {
    let (header, entries) = sddl.split_once('(').unwrap_or((sddl, ""));
    let mut entries: Vec<&str> = entries
        .trim_end_matches(')')
        .split(")(")
        .filter(|entry| !entry.is_empty())
        .collect();
    entries.sort_unstable();
    (header.starts_with("D:") && header.contains('P'), entries)
}
