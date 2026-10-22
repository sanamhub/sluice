//! The local checks of `sluice doctor`: the home, who can reach it, its layout, its lock, the
//! identity key and the free space on its volume.
//!
//! Every check only reads. None creates, locks for longer than a try, repairs or deletes anything,
//! so it can be run on a home that is in trouble without adding to the trouble. The checks return
//! what they found as data, and the command decides how that reads and which step fixes it.

mod checks;
#[cfg(test)]
mod tests;

use std::env;
use std::path::{Path, PathBuf};

use super::Home;
use crate::ErrorKind;

/// Below this many free bytes on the home's volume, the free space check is a warning.
///
/// # Examples
///
/// ```
/// use sluice_core::doctor::LOW_SPACE_BYTES;
///
/// assert_eq!(LOW_SPACE_BYTES, 2 * 1024 * 1024 * 1024);
/// ```
pub const LOW_SPACE_BYTES: u64 = 2 << 30;

/// How a check came out.
///
/// # Examples
///
/// ```
/// use sluice_core::doctor::{Check, Finding, Status};
///
/// assert_eq!(Check::new(Finding::LockFree).status(), Status::Ok);
/// assert_eq!(Check::new(Finding::KeyMissing).status(), Status::Warning);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing to do.
    Ok,
    /// Sluice works, and something is still worth doing, or the next normal start does it.
    Warning,
    /// Sluice does not work until the user does something.
    Failed,
    /// Not looked at, because the home it needs is not there.
    Skipped,
}

/// Which check a finding belongs to, in the order the checks run.
///
/// # Examples
///
/// ```
/// use sluice_core::doctor::{Check, Finding, Name};
///
/// assert_eq!(Check::new(Finding::SpaceFree(1 << 40)).name(), Name::Space);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Name {
    /// The home folder.
    Home,
    /// Who other than this user can reach the home folder.
    Access,
    /// The layout version in `VERSION`.
    Layout,
    /// The one-writer lock.
    Lock,
    /// The identity key.
    Key,
    /// The free space on the home's volume.
    Space,
}

/// What a check found. Nothing in it holds a byte of the key.
///
/// # Examples
///
/// ```
/// use sluice_core::doctor::Finding;
///
/// let finding = Finding::LayoutNewer { found: 2, known: 1 };
/// assert_ne!(finding, Finding::LayoutCurrent(1));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// The home and every folder of its layout are there.
    HomeReady(Location),
    /// Nothing is at the home's path. Any command that opens the store creates it.
    HomeMissing(Location),
    /// Something that is not a folder is at the home's path.
    HomeNotAFolder(Location),
    /// The home is there, and these folders of its layout are not.
    HomeFoldersMissing(Location, Vec<&'static str>),
    /// The home's path cannot be looked at. Carries why.
    HomeUnreadable(Location, String),
    /// There is no `$SLUICE_HOME`, and this user has no data folder to put a home in.
    HomeNowhere,
    /// `VERSION` holds the version this build writes.
    LayoutCurrent(u32),
    /// `VERSION` holds an older version, which the next normal start migrates.
    LayoutOlder {
        /// The version in the file.
        found: u32,
        /// The version this build writes.
        current: u32,
    },
    /// There is no `VERSION`. The next normal start writes the current one.
    LayoutNoFile(u32),
    /// `VERSION` holds a version newer than this build reads. It does not write to that home.
    LayoutNewer {
        /// The version in the file.
        found: u32,
        /// The newest version this build reads.
        known: u32,
    },
    /// `VERSION` cannot be read or does not hold a version. Carries why.
    LayoutUnreadable(String),
    /// No process holds the lock, and the last one let it go cleanly.
    LockFree,
    /// A running process holds the lock: the one writer the store allows. Carries its pid when
    /// `lock.pid` names one.
    LockHeld(Option<u32>),
    /// No process holds the lock, and `lock.pid` is still there, so the last holder ended without
    /// letting go cleanly. The next normal start reclaims it. Carries the pid in the file.
    LockStale(Option<u32>),
    /// The lock cannot be opened or tried. Carries why.
    LockUnreadable(String),
    /// The key is there, 32 bytes long, and only this user can read it.
    KeyPrivate,
    /// There is no key yet. `sluice id` makes one.
    KeyMissing,
    /// The key is not 32 bytes long. Carries its length.
    KeyWrongSize(u64),
    /// On Unix, group or other has access to the key. Carries its mode.
    KeyOpenToOthers(u32),
    /// Only this user can reach the home folder: mode 0700 on Unix, and on Windows the private
    /// access list the store gives a new home.
    AccessPrivate,
    /// On Unix, group or other has access to the home folder. Carries its mode.
    HomeOpenToOthers(u32),
    /// On Windows, the key's access list is not the private one the store sets: the user,
    /// `SYSTEM` and `Administrators`, with inheritance stopped.
    KeyAclNotPrivate,
    /// On Windows, the home folder's access list is not the private one the store sets on a new
    /// home: the same three entries, inherited by what is made inside, with inheritance from the
    /// folder above stopped.
    HomeAclNotPrivate,
    /// The home folder's mode or access list cannot be read. Carries why.
    AccessUnreadable(String),
    /// The key or its access cannot be read. Carries why.
    KeyUnreadable(String),
    /// Bytes free to this user on the home's volume.
    SpaceFree(u64),
    /// The volume cannot be asked how much is free. Carries why.
    SpaceUnreadable(String),
    /// Not looked at, because the home is not there.
    NeedsHome(Name),
}

/// Where the home is, and whether `$SLUICE_HOME` said so.
///
/// # Examples
///
/// ```
/// use sluice_core::doctor::Location;
///
/// let location = Location { path: "/srv/sluice".into(), from_env: true };
/// assert!(location.from_env);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The folder of the home.
    pub path: PathBuf,
    /// Whether `$SLUICE_HOME` named it, rather than the default.
    pub from_env: bool,
}

/// One finding, with the check it belongs to.
///
/// # Examples
///
/// ```
/// use sluice_core::ErrorKind;
/// use sluice_core::doctor::{Check, Finding, Status};
///
/// let check = Check::new(Finding::KeyOpenToOthers(0o644));
/// assert_eq!((check.status(), check.failure()), (Status::Failed, Some(ErrorKind::Policy)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    finding: Finding,
}

impl Check {
    /// The check for `finding`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::doctor::{Check, Finding};
    ///
    /// assert_eq!(Check::new(Finding::KeyPrivate).finding(), &Finding::KeyPrivate);
    /// ```
    pub fn new(finding: Finding) -> Self {
        Self { finding }
    }

    /// What was found.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::doctor::{Check, Finding};
    ///
    /// assert_eq!(Check::new(Finding::LockFree).finding(), &Finding::LockFree);
    /// ```
    pub fn finding(&self) -> &Finding {
        &self.finding
    }

    /// Which check this is.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::doctor::{Check, Finding, Name};
    ///
    /// assert_eq!(Check::new(Finding::NeedsHome(Name::Key)).name(), Name::Key);
    /// ```
    pub fn name(&self) -> Name {
        use Finding as F;
        match &self.finding {
            F::HomeReady(_)
            | F::HomeMissing(_)
            | F::HomeNotAFolder(_)
            | F::HomeFoldersMissing(..)
            | F::HomeUnreadable(..)
            | F::HomeNowhere => Name::Home,
            F::AccessPrivate
            | F::HomeOpenToOthers(_)
            | F::HomeAclNotPrivate
            | F::AccessUnreadable(_) => Name::Access,
            F::LayoutCurrent(_)
            | F::LayoutOlder { .. }
            | F::LayoutNoFile(_)
            | F::LayoutNewer { .. }
            | F::LayoutUnreadable(_) => Name::Layout,
            F::LockFree | F::LockHeld(_) | F::LockStale(_) | F::LockUnreadable(_) => Name::Lock,
            F::KeyPrivate
            | F::KeyMissing
            | F::KeyWrongSize(_)
            | F::KeyOpenToOthers(_)
            | F::KeyAclNotPrivate
            | F::KeyUnreadable(_) => Name::Key,
            F::SpaceFree(_) | F::SpaceUnreadable(_) => Name::Space,
            F::NeedsHome(name) => *name,
        }
    }

    /// How the check came out.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::doctor::{Check, Finding, LOW_SPACE_BYTES, Status};
    ///
    /// assert_eq!(Check::new(Finding::SpaceFree(LOW_SPACE_BYTES - 1)).status(), Status::Warning);
    /// assert_eq!(Check::new(Finding::SpaceFree(LOW_SPACE_BYTES)).status(), Status::Ok);
    /// ```
    pub fn status(&self) -> Status {
        use Finding as F;
        if self.failure().is_some() {
            return Status::Failed;
        }
        match &self.finding {
            F::LayoutOlder { .. }
            | F::LayoutNoFile(_)
            | F::LockStale(_)
            | F::KeyMissing
            | F::SpaceUnreadable(_) => Status::Warning,
            F::SpaceFree(free) if *free < LOW_SPACE_BYTES => Status::Warning,
            F::NeedsHome(_) => Status::Skipped,
            _ => Status::Ok,
        }
    }

    /// The kind of error a failed check is, for the exit code of `sluice doctor`: policy when
    /// sluice refuses by rule, such as a key other users can read, and storage when the files are
    /// not what they must be. `None` for a check that did not fail.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::ErrorKind;
    /// use sluice_core::doctor::{Check, Finding};
    ///
    /// assert_eq!(Check::new(Finding::KeyWrongSize(5)).failure(), Some(ErrorKind::Storage));
    /// assert_eq!(Check::new(Finding::KeyMissing).failure(), None);
    /// ```
    pub fn failure(&self) -> Option<ErrorKind> {
        use Finding as F;
        match &self.finding {
            F::HomeMissing(_)
            | F::HomeNotAFolder(_)
            | F::HomeFoldersMissing(..)
            | F::HomeUnreadable(..)
            | F::HomeNowhere
            | F::LayoutUnreadable(_)
            | F::LockUnreadable(_)
            | F::AccessUnreadable(_)
            | F::KeyWrongSize(_)
            | F::KeyUnreadable(_) => Some(ErrorKind::Storage),
            F::LayoutNewer { .. }
            | F::KeyOpenToOthers(_)
            | F::HomeOpenToOthers(_)
            | F::KeyAclNotPrivate
            | F::HomeAclNotPrivate => Some(ErrorKind::Policy),
            _ => None,
        }
    }
}

/// Runs every local check on the home sluice would use: `$SLUICE_HOME` if it is set and not
/// empty, else the default. The checks come in the order of [`Name`].
///
/// # Examples
///
/// ```
/// use sluice_core::doctor::{Name, examine};
///
/// let names: Vec<Name> = examine().iter().map(|check| check.name()).collect();
/// let all = [Name::Home, Name::Access, Name::Layout, Name::Lock, Name::Key, Name::Space];
/// assert_eq!(names, all);
/// ```
pub fn examine() -> Vec<Check> {
    let from_env = env::var_os("SLUICE_HOME").is_some_and(|value| !value.is_empty());
    match Home::locate() {
        Ok(path) => examine_at(&path, from_env),
        Err(_) => checks::without_home(Finding::HomeNowhere),
    }
}

/// Runs every local check on the home at `path`. `from_env` says whether `$SLUICE_HOME` named it,
/// which the report repeats.
///
/// # Examples
///
/// ```
/// use sluice_core::Home;
/// use sluice_core::doctor::{Finding, Status, examine_at};
///
/// let dir = tempfile::tempdir()?;
/// let checks = examine_at(&dir.path().join("none"), false);
/// assert!(matches!(checks[0].finding(), Finding::HomeMissing(_)));
/// assert!(checks[1..].iter().all(|check| check.status() == Status::Skipped));
/// assert!(!dir.path().join("none").exists(), "nothing was created");
///
/// let home = dir.path().join("home");
/// Home::open(&home)?.identity()?;
/// let checks = examine_at(&home, true);
/// assert!(checks[..5].iter().all(|check| check.status() == Status::Ok), "{checks:?}");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn examine_at(path: &Path, from_env: bool) -> Vec<Check> {
    let location = Location {
        path: path.to_path_buf(),
        from_env,
    };
    let home = checks::home(location);
    if !matches!(
        home,
        Finding::HomeReady(_) | Finding::HomeFoldersMissing(..)
    ) {
        return checks::without_home(home);
    }
    // The checks read the folder the files really land in, as `Home::open` writes them: a junction
    // or a symlink given as the home has an access list or a mode of its own, which says nothing
    // about its target's. The report names the path as given.
    let real = super::setup::resolve(path).unwrap_or_else(|_| path.to_path_buf());
    vec![
        Check::new(home),
        Check::new(checks::access(&real, path)),
        Check::new(checks::layout(&real)),
        Check::new(checks::lock(&real)),
        Check::new(checks::key(&real)),
        Check::new(checks::space(&real)),
    ]
}
