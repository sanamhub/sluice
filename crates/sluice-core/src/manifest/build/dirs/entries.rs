//! Reading the names out of a buffer the system filled with `FILE_ID_BOTH_DIR_INFO` entries, which is
//! how a Windows folder is listed from its handle.

#[cfg(any(windows, test))]
use std::ffi::OsString;
#[cfg(any(windows, test))]
use std::io;

/// Where the fields of a `FILE_ID_BOTH_DIR_INFO` lie, which is how the system lays it out: the offset
/// of the next entry, the length of the name in bytes, and the name itself.
#[cfg(any(windows, test))]
pub(super) const NEXT_AT: usize = 0;
#[cfg(any(windows, test))]
pub(super) const LENGTH_AT: usize = 60;
#[cfg(any(windows, test))]
pub(super) const NAME_AT: usize = 104;

#[cfg(windows)]
const _: () = {
    use windows_sys::Win32::Storage::FileSystem::FILE_ID_BOTH_DIR_INFO;
    assert!(std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, NextEntryOffset) == NEXT_AT);
    assert!(std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileNameLength) == LENGTH_AT);
    assert!(std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName) == NAME_AT);
};

/// Reads the names out of a buffer the system filled with `FILE_ID_BOTH_DIR_INFO` entries, and
/// refuses a buffer that is not well formed: a name that does not lie inside it, an odd name
/// length, or a step to the next entry that is not a multiple of eight, is shorter than an entry's
/// fixed part, or leaves the buffer. It reads bytes and makes no reference to the structure, so a
/// bad value is an error and never a read outside the buffer.
#[cfg(any(windows, test))]
pub(super) fn parse_entries(buffer: &[u8], names: &mut Vec<OsString>) -> io::Result<()> {
    let bad = || io::Error::other("the system gave a folder listing that is not well formed");
    let word = |at: usize| -> io::Result<usize> {
        let bytes = buffer
            .get(at..at.checked_add(4).ok_or_else(bad)?)
            .ok_or_else(bad)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    };
    let mut at = 0usize;
    loop {
        let next = word(at.checked_add(NEXT_AT).ok_or_else(bad)?)?;
        let length = word(at.checked_add(LENGTH_AT).ok_or_else(bad)?)?;
        let start = at.checked_add(NAME_AT).ok_or_else(bad)?;
        let end = start.checked_add(length).ok_or_else(bad)?;
        if !length.is_multiple_of(2) || end > buffer.len() {
            return Err(bad());
        }
        let units: Vec<u16> = buffer[start..end]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let name = os_name(&units);
        if name != "." && name != ".." {
            names.push(name);
        }
        if next == 0 {
            return Ok(());
        }
        let after = at.checked_add(next).ok_or_else(bad)?;
        if !next.is_multiple_of(8) || next < NAME_AT || after >= buffer.len() {
            return Err(bad());
        }
        at = after;
    }
}

#[cfg(windows)]
fn os_name(units: &[u16]) -> OsString {
    std::os::windows::ffi::OsStringExt::from_wide(units)
}

#[cfg(all(test, not(windows)))]
fn os_name(units: &[u16]) -> OsString {
    OsString::from(String::from_utf16_lossy(units))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entry of the layout the system gives, with the name at the fixed offset.
    fn entry(next: u32, name: &str, length: Option<u32>) -> Vec<u8> {
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut bytes = vec![0u8; NAME_AT];
        bytes[NEXT_AT..NEXT_AT + 4].copy_from_slice(&next.to_le_bytes());
        let length = length.unwrap_or(units.len() as u32);
        bytes[LENGTH_AT..LENGTH_AT + 4].copy_from_slice(&length.to_le_bytes());
        bytes.extend(units);
        bytes
    }

    fn pad(mut bytes: Vec<u8>) -> Vec<u8> {
        while !bytes.len().is_multiple_of(8) {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn a_well_formed_buffer_gives_its_names_and_skips_dot_and_dot_dot() {
        let first = pad(entry(0, ".", None));
        let second = pad(entry(0, "..", None));
        let third = pad(entry(0, "ünï.txt", None));
        let mut buffer = Vec::new();
        for (index, part) in [first, second, third].into_iter().enumerate() {
            let mut part = part;
            if index < 2 {
                let next = part.len() as u32;
                part[NEXT_AT..NEXT_AT + 4].copy_from_slice(&next.to_le_bytes());
            }
            buffer.extend(part);
        }
        let mut names = Vec::new();
        parse_entries(&buffer, &mut names).unwrap();
        assert_eq!(names, [OsString::from("ünï.txt")]);
    }

    #[test]
    fn a_bad_step_to_the_next_entry_is_an_error() {
        let mut names = Vec::new();
        let body = pad(entry(0, "a", None));
        let long = body.len() + 64;
        for next in [
            4u32,                 // not a multiple of eight
            8,                    // inside the fixed part of the entry
            (NAME_AT - 8) as u32, // a multiple of eight, short of the fixed part
            long as u32,          // beyond the buffer
            !7u32,                // far beyond it
        ] {
            let mut buffer = body.clone();
            buffer[NEXT_AT..NEXT_AT + 4].copy_from_slice(&next.to_le_bytes());
            assert!(parse_entries(&buffer, &mut names).is_err(), "step {next}");
        }
        assert!(names.len() <= 5, "no unbounded growth");
    }

    #[test]
    fn a_bad_name_length_is_an_error() {
        let mut names = Vec::new();
        let buffer = pad(entry(0, "abc", Some(5000)));
        assert!(
            parse_entries(&buffer, &mut names).is_err(),
            "past the buffer"
        );
        let buffer = pad(entry(0, "abc", Some(5)));
        assert!(parse_entries(&buffer, &mut names).is_err(), "odd");
        let buffer = pad(entry(0, "abc", Some(u32::MAX)));
        assert!(parse_entries(&buffer, &mut names).is_err(), "huge");
        assert!(
            parse_entries(&[0u8; 40], &mut names).is_err(),
            "shorter than a header"
        );
    }
}
