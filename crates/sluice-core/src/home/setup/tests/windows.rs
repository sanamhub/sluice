//! The Windows half: a hidden `desktop.ini`, which Explorer writes into a folder whose look the
//! user changed, makes a folder not empty.

use std::fs;

use super::{is_private, open_folder};
use crate::Home;

#[test]
fn a_folder_that_holds_only_a_hidden_desktop_ini_is_not_new_and_is_left_alone() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    open_folder(&path);
    let ini = path.join("desktop.ini");
    fs::write(&ini, "[.ShellClassInfo]\r\n").unwrap();
    // The hidden attribute, as Explorer sets it. `attrib` needs no right a user lacks.
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&ini)
        .status()
        .unwrap();
    assert!(status.success());
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
        let attributes = fs::metadata(&ini).unwrap().file_attributes();
        assert_ne!(attributes & FILE_ATTRIBUTE_HIDDEN, 0, "the file is hidden");
    }
    drop(Home::open(&path).unwrap());
    assert!(
        !is_private(&path),
        "a folder that holds a hidden file keeps its access"
    );
}
