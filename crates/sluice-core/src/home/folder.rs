//! The per-user data folder, read from the environment.
//!
//! The `dirs` and `directories` crates answer this too, but both depend on `option-ext`, which is
//! MPL-2.0, and `deny.toml` does not allow that license. Three environment variables are all the
//! answer needs on the platforms we build for, and they are what those crates read first.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

/// The folder the platform keeps a user's application data in, or `None` if the environment does
/// not say. Linux and the other Unix systems use `$XDG_DATA_HOME`, or `~/.local/share`. macOS uses
/// `~/Library/Application Support`. Windows uses the local app data folder, and not the roaming
/// one, so the identity key never syncs to another machine.
pub(super) fn data_folder() -> Option<PathBuf> {
    if cfg!(windows) {
        windows(env::var_os("LOCALAPPDATA"))
    } else if cfg!(target_os = "macos") {
        macos(env::var_os("HOME"))
    } else {
        xdg(env::var_os("XDG_DATA_HOME"), env::var_os("HOME"))
    }
}

/// A value that is an absolute path. The XDG specification says to ignore a relative
/// `$XDG_DATA_HOME`, and a relative `$HOME` would put the identity key wherever the process
/// happens to be running.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_absolute())
}

fn xdg(data_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    absolute(data_home).or_else(|| absolute(home).map(|home| home.join(".local").join("share")))
}

fn macos(home: Option<OsString>) -> Option<PathBuf> {
    absolute(home).map(|home| home.join("Library").join("Application Support"))
}

fn windows(local_app_data: Option<OsString>) -> Option<PathBuf> {
    absolute(local_app_data)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::{data_folder, macos, windows, xdg};

    /// An absolute path on every platform, which `/x` is not on Windows.
    fn abs(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    fn os(path: &Path) -> Option<OsString> {
        Some(path.as_os_str().to_owned())
    }

    #[test]
    fn xdg_data_home_wins_and_home_is_the_fallback() {
        let (data, home) = (abs("data"), abs("home"));
        assert_eq!(xdg(os(&data), os(&home)), Some(data));
        assert_eq!(
            xdg(None, os(&home)),
            Some(home.join(".local").join("share"))
        );
    }

    #[test]
    fn a_relative_or_empty_xdg_data_home_is_ignored() {
        let home = abs("home");
        let fallback = Some(home.join(".local").join("share"));
        for bad in [Some(OsString::from("relative/dir")), Some(OsString::new())] {
            assert_eq!(xdg(bad, os(&home)), fallback);
        }
    }

    #[test]
    fn without_an_absolute_home_there_is_no_folder() {
        assert_eq!(xdg(None, None), None);
        assert_eq!(xdg(None, Some(OsString::from("relative"))), None);
        assert_eq!(macos(None), None);
        assert_eq!(macos(Some(OsString::from("relative"))), None);
        assert_eq!(windows(None), None);
        assert_eq!(windows(Some(OsString::from("relative"))), None);
    }

    #[test]
    fn macos_uses_application_support_and_windows_uses_the_local_folder() {
        let home = abs("home");
        assert_eq!(
            macos(os(&home)),
            Some(home.join("Library").join("Application Support"))
        );
        let local = abs("local");
        assert_eq!(windows(os(&local)), Some(local));
    }

    #[test]
    fn the_real_environment_gives_an_absolute_folder_or_nothing() {
        // Reads the environment and never changes it, so it cannot race another test.
        if let Some(folder) = data_folder() {
            assert!(folder.is_absolute(), "{}", folder.display());
        }
    }
}
