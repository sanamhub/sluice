use std::fs;

use iroh_blobs::Hash;

use super::{copy_and_hash, repair};
use crate::engine::storage::identify;

/// A reader whose second read gives other bytes, the way a source file edited through its other
/// name does while it is copied. Only a copy that hashes the bytes it wrote can see that.
struct Changing {
    reads: u8,
}

impl std::io::Read for Changing {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.reads += 1;
        let bytes: &[u8] = match self.reads {
            1 => b"one",
            2 => b"two",
            _ => b"",
        };
        out[..bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
}

/// The bytes a copy is judged by are the ones it wrote. A source that changed after the first read
/// gives a copy that is not the blob of the root, and nothing is installed from it.
#[test]
fn a_copy_is_judged_by_the_bytes_it_wrote() {
    let mut written = Vec::new();
    let root = Hash::new(b"one");
    let copied = copy_and_hash(&mut Changing { reads: 0 }, &mut written, root).unwrap();
    assert!(
        !copied,
        "a copy whose source changed was taken for the blob"
    );
    assert_eq!(written, b"onetwo");
}

/// A store file with a second name is copied and the copy takes its place, so the store stops
/// sharing its bytes with the output and keeps them.
#[cfg(unix)]
#[tokio::test]
async fn a_shared_data_file_is_given_its_own_bytes() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    let data = store.join("data");
    fs::create_dir_all(&data).unwrap();
    let body = b"the store's bytes";
    let hash = Hash::new(body);
    let source = data.join(format!("{hash}.data"));
    fs::write(&source, body).unwrap();
    fs::hard_link(&source, dir.path().join("output")).unwrap();

    repair(&store, identify(&store).unwrap()).await.unwrap();
    assert_eq!(fs::read(&source).unwrap(), body);
    assert_eq!(fs::read(dir.path().join("output")).unwrap(), body);
    assert_eq!(
        fs::metadata(&source).unwrap().nlink(),
        1,
        "the store still shares its bytes"
    );
    assert!(!data.join(format!("{hash}.store")).exists());
}

/// A link where the store keeps its data files is not followed: what it leads to keeps its names
/// and its bytes, and the store is refused rather than repaired through it.
#[cfg(unix)]
#[tokio::test]
async fn a_link_where_the_data_folder_belongs_is_refused() {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    fs::create_dir(&store).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let body = b"outside bytes";
    let hash = Hash::new(body);
    let outside_data = outside.join(format!("{hash}.data"));
    fs::write(&outside_data, body).unwrap();
    fs::hard_link(&outside_data, outside.join("second")).unwrap();
    std::os::unix::fs::symlink(&outside, store.join("data")).unwrap();

    let error = repair(&store, identify(&store).unwrap()).await.unwrap_err();
    assert!(matches!(error, crate::Error::Policy(_)), "{error}");
    assert_eq!(fs::read(&outside_data).unwrap(), body);
    assert_eq!(fs::metadata(&outside_data).unwrap().nlink(), 2);
    assert!(
        !outside.join(format!("{hash}.store")).exists(),
        "the link was followed"
    );
}

/// A link planted at the copy's name is taken away and never written through, and the repair still
/// gives the data file a name of its own.
#[cfg(unix)]
#[tokio::test]
async fn a_link_at_a_copy_name_is_not_written_through() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    let data = store.join("data");
    fs::create_dir_all(&data).unwrap();
    let body = b"the store's bytes";
    let hash = Hash::new(body);
    let source = data.join(format!("{hash}.data"));
    fs::write(&source, body).unwrap();
    fs::hard_link(&source, dir.path().join("output")).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, b"untouched").unwrap();
    let copy = data.join(format!("{hash}.store"));
    std::os::unix::fs::symlink(&outside, &copy).unwrap();

    repair(&store, identify(&store).unwrap()).await.unwrap();
    assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    assert!(!copy.exists(), "the link stayed");
}

/// The same check on a junction, which Windows lets a user make without a privilege.
#[cfg(windows)]
#[tokio::test]
async fn a_junction_where_the_data_folder_belongs_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    fs::create_dir(&store).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let body = b"outside bytes";
    let hash = Hash::new(body);
    let outside_data = outside.join(format!("{hash}.data"));
    fs::write(&outside_data, body).unwrap();
    fs::hard_link(&outside_data, outside.join("second")).unwrap();

    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(store.join("data"))
        .arg(&outside)
        .output();
    if !made.is_ok_and(|output| output.status.success()) {
        eprintln!("SKIPPED: this account cannot make a junction");
        return;
    }

    let error = repair(&store, identify(&store).unwrap()).await.unwrap_err();
    assert!(matches!(error, crate::Error::Policy(_)), "{error}");
    assert_eq!(fs::read(&outside_data).unwrap(), body);
    let names = super::name_count(&outside_data, &fs::metadata(&outside_data).unwrap()).unwrap();
    assert_eq!(names, 2);
    assert!(
        !outside.join(format!("{hash}.store")).exists(),
        "the junction was followed"
    );
}
