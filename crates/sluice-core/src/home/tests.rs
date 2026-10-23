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

mod as_given {
    use std::path::Path;

    use super::super::{as_given, show};
    use crate::Error;

    #[test]
    fn the_resolved_path_is_written_as_the_caller_gave_it() {
        let text = "cannot open /srv/real/lock: denied, and /srv/real/blobs too";
        let shown = show(text, Path::new("/srv/real"), Path::new("/home/me/link"));
        assert_eq!(
            shown,
            "cannot open /home/me/link/lock: denied, and /home/me/link/blobs too"
        );
        // Nothing to change when they are one path.
        assert_eq!(
            show(text, Path::new("/srv/real"), Path::new("/srv/real")),
            text
        );
    }

    /// A caller who ends the path with a separator would otherwise get `home//blobs`.
    #[test]
    fn a_separator_at_the_end_of_the_given_path_is_not_doubled() {
        let text = "cannot open /srv/real/lock";
        for given in ["/home/me/link/", "/home/me/link//"] {
            assert_eq!(
                show(text, Path::new("/srv/real"), Path::new(given)),
                "cannot open /home/me/link/lock",
                "{given}"
            );
        }
        // A path that is nothing but a separator keeps it.
        assert_eq!(show("at /a/b", Path::new("/a/b"), Path::new("/")), "at /");
    }

    /// `/h/../h` holds the resolved `/h`, and is put in once: a second pass over the result would
    /// find `/h` inside it again.
    #[test]
    fn a_given_path_that_holds_the_resolved_one_is_not_replaced_again() {
        let shown = show("cannot open /h/lock", Path::new("/h"), Path::new("/h/../h"));
        assert_eq!(shown, "cannot open /h/../h/lock");
        let once = Error::Storage("cannot open /h/lock".to_string());
        let Error::Storage(message) = as_given(once, Path::new("/h"), Path::new("/h/../h")) else {
            panic!("the kind changed");
        };
        assert_eq!(message, "cannot open /h/../h/lock");
    }

    /// Only a whole path is replaced: `/h` is not a part of `/hx` or of `/other/h`.
    #[test]
    fn another_path_that_starts_or_ends_alike_is_left_alone() {
        let text = "/hx/lock /other/h/lock /h/lock /h";
        let shown = show(text, Path::new("/h"), Path::new("/g"));
        assert_eq!(shown, "/hx/lock /other/h/lock /g/lock /g");
    }

    /// A separator or the end of the text is the only boundary, so a name that goes on with a dot
    /// or a letter is another folder and is not replaced.
    #[test]
    fn a_path_that_goes_on_with_a_name_is_not_the_resolved_one() {
        let text = "/srv/real.old/lock /srv/realx/lock /srv/real/lock";
        let shown = show(text, Path::new("/srv/real"), Path::new("/g"));
        assert_eq!(shown, "/srv/real.old/lock /srv/realx/lock /g/lock");
    }

    #[test]
    fn only_a_storage_or_policy_message_is_rewritten() {
        let shown = as_given(Error::Cancelled, Path::new("/h"), Path::new("/g"));
        assert!(matches!(shown, Error::Cancelled));
        let policy = Error::Policy("busy: /h".to_string());
        let Error::Policy(message) = as_given(policy, Path::new("/h"), Path::new("/g")) else {
            panic!("the kind changed");
        };
        assert_eq!(message, "busy: /g");
    }
}
