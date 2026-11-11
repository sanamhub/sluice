//! The root a send of a path names, rebuilt without importing or serving anything. This is the
//! same code the send uses: the one-entry manifest of a file, or the built manifest of a folder,
//! and the hash-sequence root of it. A check that copied that arithmetic would agree with the send
//! and with nothing else.

use std::fs;
use std::path::Path;

use iroh_blobs::Hash;

use super::single::{checked_file, one_entry_manifest};
use crate::Error;
use crate::manifest::{Build, Kind, Manifest, path::shown};

/// The root a send of one path names, its length, and how many files it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathRoot {
    /// The hash-sequence root, which is what a static ticket's `root` is.
    pub root: [u8; 32],
    /// The sum of the file sizes, which is what the ticket's `size` is.
    pub total_len: u64,
    /// How many files a receiver would write.
    pub files: u64,
}

/// The root a send of `path` names, read from the path as it is now.
///
/// A regular file takes the single-file shape `sluice send` gives it, through
/// [`one_entry_manifest`], and a folder takes the shape of a folder argument. A symlink to a file is
/// refused, as the send refuses it. A name a receiver would reject fails with [`Error::Policy`],
/// and a file that changed while it was read with [`Error::Verify`].
pub fn root_of_path(path: &Path) -> Result<PathRoot, Error> {
    let meta = fs::metadata(path).map_err(|e| {
        Error::Storage(format!(
            "cannot read {}: {e}",
            shown(&path.display().to_string())
        ))
    })?;
    if meta.is_file() {
        one_file(path)
    } else if meta.is_dir() {
        folder(path)
    } else {
        Err(Error::Policy(format!(
            "{} is not a file or a folder",
            shown(&path.display().to_string())
        )))
    }
}

/// The root of a single-file send: its name, size, time and mode in a one-entry manifest, hashed as
/// the send hashes it.
fn one_file(path: &Path) -> Result<PathRoot, Error> {
    let (label, _name, entry_path, before) = checked_file(path)?;
    let (hash, len) = hash_file(path, &label)?;
    if len != before.size || !before.same(path)? {
        return Err(Error::Verify(format!(
            "{label} changed while it was read, so its root names no version of it"
        )));
    }
    let manifest = one_entry_manifest(entry_path, &before, Hash::from(hash))?;
    Ok(PathRoot {
        root: manifest.sequence_root(),
        total_len: before.size,
        files: 1,
    })
}

/// The root of a folder send, through the manifest builder the send uses.
fn folder(path: &Path) -> Result<PathRoot, Error> {
    let Build::Ready(built) = Manifest::build_paths(&[path.to_path_buf()])? else {
        return Err(Error::Policy(
            "a path cannot be checked because a receiver would reject one of its names".to_string(),
        ));
    };
    let manifest = built.manifest;
    let total_len = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .map(|entry| entry.size)
        .sum();
    let files = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .count() as u64;
    Ok(PathRoot {
        root: manifest.sequence_root(),
        total_len,
        files,
    })
}

/// The BLAKE3 root of the file's bytes, which is the root `iroh-blobs` gives the same bytes. The
/// size is read as it goes, so a file that grew is seen without a second pass.
fn hash_file(path: &Path, shown: &str) -> Result<([u8; 32], u64), Error> {
    let mut file =
        fs::File::open(path).map_err(|e| Error::Storage(format!("cannot open {shown}: {e}")))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut len = 0u64;
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|e| Error::Storage(format!("cannot read {shown}: {e}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        len += read as u64;
    }
    Ok((*hasher.finalize().as_bytes(), len))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{PathRoot, root_of_path};

    /// One file goes through the single-file shape, and the root it names is the one the send of
    /// the same bytes names.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_root_is_the_sequence_root_of_its_one_entry_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path: &Path = &dir.path().join("note.txt");
        fs::write(path, b"some bytes").unwrap();
        let PathRoot {
            root,
            total_len,
            files,
        } = root_of_path(path).unwrap();
        assert_eq!(total_len, 10);
        assert_eq!(files, 1);

        let home = crate::Home::open(&dir.path().join("home")).unwrap();
        let outgoing = crate::engine::prepare_send(
            &home,
            path,
            crate::engine::RelayModeChoice::Disabled,
            crate::engine::Congestion::Cubic,
            None,
            None,
        )
        .await
        .unwrap();
        let crate::ticket::Body::Static { root: sent, .. } = outgoing.ticket.body() else {
            panic!("a file send is static");
        };
        assert_eq!(*sent, root);
        assert_eq!(outgoing.bytes, total_len);
    }

    /// A folder's root is the one the same folder named on the command line gets, and its length is
    /// the sum of its files.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_folder_root_is_the_one_its_manifest_names() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("tree");
        fs::create_dir_all(folder.join("sub")).unwrap();
        fs::write(folder.join("a.txt"), b"aaaa").unwrap();
        fs::write(folder.join("sub").join("b.txt"), b"bb").unwrap();
        let root = root_of_path(&folder).unwrap();
        assert_eq!(root.files, 2);
        assert_eq!(root.total_len, 6);

        let home = crate::Home::open(&dir.path().join("home")).unwrap();
        let outgoing = crate::engine::prepare_send_paths(
            &home,
            &[folder],
            crate::engine::RelayModeChoice::Disabled,
            crate::engine::Congestion::Cubic,
            None,
            None,
        )
        .await
        .unwrap();
        let crate::ticket::Body::Static { root: sent, .. } = outgoing.ticket.body() else {
            panic!("a folder send is static");
        };
        assert_eq!(*sent, root.root);
        assert_eq!(outgoing.bytes, root.total_len);
    }

    /// The root names the name and the time as well as the bytes, so the same bytes under another
    /// name are other content.
    #[test]
    fn another_name_is_another_root() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let other = dir.path().join("other.txt");
        fs::write(&one, b"same bytes").unwrap();
        fs::write(&other, b"same bytes").unwrap();
        assert_ne!(
            root_of_path(&one).unwrap().root,
            root_of_path(&other).unwrap().root
        );
    }

    /// A name that is neither a file nor a folder is refused rather than hashed as an empty tree.
    #[test]
    fn something_that_is_not_a_file_or_a_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(root_of_path(&dir.path().join("absent")).is_err());
    }
}
