//! Making a fresh home private through one handle.
//!
//! The owner is read from a handle opened without following a link, and the mode or the access
//! list is set through that same handle. A link put at the home between the two calls would move a
//! path-based second call somewhere else; a handle names the object that was opened. On Unix that
//! is `O_DIRECTORY | O_NOFOLLOW` with `fstat`/`fchmod`; on Windows
//! `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS` with `GetSecurityInfo` and
//! `SetSecurityInfo`.

use std::path::Path;

use crate::Error;

/// The one message for a home another user owns, the same on both platforms because the fix is the
/// same: the owner can change what sluice sets, so the folder has to be this user's.
pub(super) fn owned_by_other(home: &Path) -> Error {
    Error::Policy(format!(
        "{} belongs to another user, who could change its access whatever sluice sets. Use a \
         folder of your own as the home, or take ownership of this one",
        home.display()
    ))
}

#[cfg(unix)]
pub(in crate::home) use unix::keep_home_private;
#[cfg(all(test, unix))]
pub(in crate::home) use unix::{keep_home_private_for, make_0700_with};
#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use rustix::fs::{Mode, OFlags};
    use rustix::io::Errno;

    use crate::Error;
    use crate::home::storage;

    use super::owned_by_other;

    /// Mode 0700 through the home's own handle, with the owner read from it first.
    pub(in crate::home) fn keep_home_private(home: &Path) -> Result<(), Error> {
        keep_home_private_for(home, rustix::process::geteuid().as_raw())
    }

    /// [`keep_home_private`] for the user `me`, so a test can be someone a folder does not belong
    /// to. The owner of a folder can change its mode whatever it holds, so a home another user
    /// made is refused before the mode is set.
    pub(in crate::home) fn keep_home_private_for(home: &Path, me: u32) -> Result<(), Error> {
        make_0700_with(home, me, |file| {
            rustix::fs::fchmod(file, Mode::from_raw_mode(0o700)).map_err(io::Error::from)
        })
    }

    /// [`keep_home_private_for`] with the call that changes the mode as a parameter, so a test can
    /// be a file system that refuses it, or one that says it worked and did nothing.
    pub(in crate::home) fn make_0700_with(
        home: &Path,
        me: u32,
        change: fn(&File) -> io::Result<()>,
    ) -> Result<(), Error> {
        let flags = OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let file =
            File::from(rustix::fs::open(home, flags, Mode::empty()).map_err(|e| opened(home, e))?);
        let before =
            rustix::fs::fstat(&file).map_err(|e| storage("read", home, io::Error::from(e)))?;
        if before.st_uid != me {
            return Err(owned_by_other(home));
        }
        change(&file).map_err(|e| refused(home, &e))?;
        let after = rustix::fs::fstat(&file).map_err(|e| refused(home, &io::Error::from(e)))?;
        let mode = after.st_mode & 0o7777;
        if mode != 0o700 {
            return Err(refused(
                home,
                &io::Error::other(format!("it still has mode {mode:04o} after the change")),
            ));
        }
        Ok(())
    }

    /// A link or something that is not a folder is refused by name. Any other open failure is the
    /// refusal of a home whose access cannot be changed, which is what the handle needed the access
    /// for, and not a planted link.
    fn opened(home: &Path, errno: Errno) -> Error {
        if errno == Errno::LOOP || errno == Errno::NOTDIR {
            return Error::Policy(format!(
                "{} is a link and not a folder of the home: remove it, and sluice makes the folder",
                home.display()
            ));
        }
        refused(home, &io::Error::from(errno))
    }

    /// The refusal of a mode that could not be set or did not stick. A file system that keeps no
    /// Unix permissions, such as vfat, answers here, and the home is refused rather than left open
    /// to every user.
    fn refused(home: &Path, error: &io::Error) -> Error {
        Error::Storage(format!(
            "cannot restrict access to {}: {error}. The home must be a folder this user owns, on a \
             file system that keeps Unix permissions, which vfat and exFAT do not",
            home.display()
        ))
    }
}

#[cfg(windows)]
pub(in crate::home) use windows_impl::keep_home_private;
#[cfg(all(test, windows))]
pub(in crate::home) use windows_impl::keep_home_private_with;
#[cfg(windows)]
mod windows_impl {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use crate::Error;
    use crate::home::{private_windows, storage};

    use super::owned_by_other;

    /// The private access list through the home's own handle, with the owner read from it first.
    pub(in crate::home) fn keep_home_private(home: &Path) -> Result<(), Error> {
        keep_home_private_with(home, private_windows::is_owned_by_us_handle)
    }

    /// [`keep_home_private`] with the owner check as a parameter, so a test can be a folder another
    /// user made, which cannot be built on one machine.
    pub(in crate::home) fn keep_home_private_with(
        home: &Path,
        owner: fn(&File) -> io::Result<bool>,
    ) -> Result<(), Error> {
        let dir =
            private_windows::open_dir(home).map_err(|e| storage("restrict access to", home, e))?;
        if dir
            .metadata()
            .map_err(|e| storage("read", home, e))?
            .file_type()
            .is_symlink()
        {
            return Err(Error::Policy(format!(
                "{} is a link and not a folder of the home: remove it, and sluice makes the folder",
                home.display()
            )));
        }
        match owner(&dir) {
            Ok(true) => {}
            Ok(false) => return Err(owned_by_other(home)),
            Err(e) => return Err(storage("read the owner of", home, e)),
        }
        private_windows::restrict_dir_file(&dir)
            .map_err(|e| storage("restrict access to", home, e))?;
        match private_windows::is_private_dir_file(&dir) {
            Ok(true) => Ok(()),
            Ok(false) => Err(storage(
                "restrict access to",
                home,
                io::Error::other("the access list is not the private one after the change"),
            )),
            Err(e) => Err(storage("read the access list of", home, e)),
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) fn keep_home_private(_home: &Path) -> Result<(), Error> {
    Ok(())
}
