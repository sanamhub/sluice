//! A listing of the names directly under a folder, read from its handle: what the folder holds, by
//! name, kind and size, with a link listed and never followed.

use std::fs::File;
use std::io;

use super::dirs::{Dir, Kind};

/// What a name in a listed folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A folder.
    Folder,
    /// A symbolic link, or on Windows a junction or another name surrogate. It is not followed.
    Link,
}

/// One name directly under a listed folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEntry {
    /// The name, with any byte that is not UTF-8 replaced.
    pub name: String,
    /// What it is.
    pub kind: EntryKind,
    /// The bytes of a file, and zero for anything else.
    pub size: u64,
}

/// The entries of a folder, and whether there were more than the limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// Folders first, then by name.
    pub entries: Vec<FolderEntry>,
    /// Whether the folder holds more than `entries` has.
    pub truncated: bool,
}

/// Lists the open folder `folder`, at most `limit` names. Every name is looked at and opened from
/// the folder's handle, so a swap of the folder cannot change which one is listed. A name that is
/// neither a file, a folder nor a link, such as a socket, is left out.
pub fn list_folder(folder: File, limit: usize) -> io::Result<Listing> {
    let dir = Dir::from_file(folder);
    let mut names = dir.names()?;
    names.sort();
    let truncated = names.len() > limit;
    names.truncate(limit);
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let (kind, size) = match dir.kind(&name) {
            Ok(Kind::File) => {
                let size = dir
                    .open_file(&name)
                    .and_then(|file| file.metadata())
                    .map_or(0, |meta| meta.len());
                (EntryKind::File, size)
            }
            Ok(Kind::Dir) => (EntryKind::Folder, 0),
            Ok(Kind::Link) => (EntryKind::Link, 0),
            // Gone since it was listed, or not something to show.
            Ok(Kind::Other) | Err(_) => continue,
        };
        entries.push(FolderEntry {
            name: name.to_string_lossy().into_owned(),
            kind,
            size,
        });
    }
    entries.sort_by(|a, b| {
        let folder_first = (b.kind == EntryKind::Folder).cmp(&(a.kind == EntryKind::Folder));
        folder_first.then_with(|| a.name.cmp(&b.name))
    });
    Ok(Listing { entries, truncated })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::engine::open_folder;

    #[test]
    fn a_folder_lists_folders_first_then_names_with_sizes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), b"12345").unwrap();
        fs::write(dir.path().join("a.txt"), b"1").unwrap();
        fs::create_dir(dir.path().join("zdir")).unwrap();
        let listing = list_folder(open_folder(dir.path()).unwrap(), 100).unwrap();
        let seen: Vec<(&str, EntryKind, u64)> = listing
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind, e.size))
            .collect();
        assert_eq!(
            seen,
            [
                ("zdir", EntryKind::Folder, 0),
                ("a.txt", EntryKind::File, 1),
                ("b.txt", EntryKind::File, 5)
            ]
        );
        assert!(!listing.truncated);
    }

    #[test]
    fn a_folder_with_more_than_the_limit_says_so() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..12 {
            fs::write(dir.path().join(format!("f{n:02}")), b"x").unwrap();
        }
        let listing = list_folder(open_folder(dir.path()).unwrap(), 10).unwrap();
        assert_eq!(listing.entries.len(), 10);
        assert!(listing.truncated);
        let exact = list_folder(open_folder(dir.path()).unwrap(), 12).unwrap();
        assert!(!exact.truncated);
    }

    /// A link to the folder `target`: a symbolic link, or on Windows a junction.
    fn link_to_folder(link: &std::path::Path, target: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(made.status.success(), "mklink failed");
        }
    }

    /// A link is listed as a link, with no size, and what it leads to is not looked at.
    #[test]
    fn a_link_is_listed_and_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        let listed = dir.path().join("listed");
        fs::create_dir(&listed).unwrap();
        link_to_folder(&listed.join("door"), &outside);
        let listing = list_folder(open_folder(&listed).unwrap(), 10).unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "door");
        assert_eq!(listing.entries[0].kind, EntryKind::Link);
        assert_eq!(listing.entries[0].size, 0);
    }
}
