//! What a check of `sluice doctor` found, and where the home it looked at is.

use std::path::PathBuf;

use super::Name;

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
    AccessOpenToOthers(u32),
    /// Another user owns the home folder: the effective user on Unix, the user or `Administrators`
    /// on Windows. An owner can change who has access whatever its mode or list says, so the
    /// folder is not private to this user even if it looks it.
    AccessOwnedByOther,
    /// On Windows, the key's access list is not the private one the store sets: the user,
    /// `SYSTEM` and `Administrators`, with inheritance stopped.
    KeyAclNotPrivate,
    /// On Windows, the home folder's access list is not the private one the store sets on a new
    /// home: the same three entries, inherited by what is made inside, with inheritance from the
    /// folder above stopped.
    AccessAclNotPrivate,
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
