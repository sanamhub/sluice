use std::ffi::OsString;
use std::path::PathBuf;

use super::locate_from;
use crate::Error;

fn os(text: &str) -> Option<OsString> {
    Some(OsString::from(text))
}

#[test]
fn sluice_home_wins_when_it_is_set_and_not_empty() {
    let data = Some(PathBuf::from("/data"));
    assert_eq!(
        locate_from(os("/elsewhere"), data.clone()).unwrap(),
        PathBuf::from("/elsewhere")
    );
    // Relative or odd, it is the user's word for it.
    assert_eq!(
        locate_from(os("rel/home"), data).unwrap(),
        PathBuf::from("rel/home")
    );
}

#[test]
fn an_empty_or_missing_sluice_home_falls_back_to_the_data_folder() {
    let data = Some(PathBuf::from("/data"));
    for sluice_home in [None, os("")] {
        let found = locate_from(sluice_home, data.clone()).unwrap();
        assert_eq!(found, PathBuf::from("/data").join("sluice"));
    }
}

#[test]
fn no_data_folder_and_no_sluice_home_is_a_storage_error_that_says_what_to_do() {
    let Err(Error::Storage(message)) = locate_from(None, None) else {
        panic!("expected a storage error");
    };
    assert!(message.contains("SLUICE_HOME"), "{message}");
}
