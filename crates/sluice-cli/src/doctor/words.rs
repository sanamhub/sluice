//! What each finding means in words, and the step that fixes it.

use sluice_core::doctor::{self, Finding, Location};

pub(super) fn words(finding: &Finding) -> (String, Option<String>) {
    use Finding as F;
    match finding {
        F::HomeReady(at) => (where_from(at), None),
        F::HomeMissing(at) => (
            format!("{} does not exist", at.path.display()),
            Some("run `sluice id`, which creates it".to_string()),
        ),
        F::HomeNotAFolder(at) => (
            format!("{} is not a folder", at.path.display()),
            Some(format!(
                "move what is at {} elsewhere, or set SLUICE_HOME to another folder",
                at.path.display()
            )),
        ),
        F::HomeFoldersMissing(at, missing) => (
            format!("{} has no {} folder", at.path.display(), missing.join(", ")),
            Some("run `sluice id`, which creates the missing folders".to_string()),
        ),
        F::HomeFolderIsLink(at, folder) => (
            format!(
                "{} is a link, not a folder of the home",
                at.path.join(folder).display()
            ),
            Some("remove it, and run `sluice id`, which makes the folder".to_string()),
        ),
        F::HomeFolderOwnedByOther(at, folder) => (
            format!(
                "{} belongs to another user, who could change it whatever sluice sets",
                at.path.join(folder).display()
            ),
            Some("take ownership of it, or set SLUICE_HOME to another folder".to_string()),
        ),
        F::HomeUnreadable(at, why) => (
            format!("{} cannot be looked at: {why}", at.path.display()),
            Some(format!(
                "check the permissions of {} and the folders above it",
                at.path.display()
            )),
        ),
        F::HomeNowhere => (
            "SLUICE_HOME is not set, and this user has no data folder".to_string(),
            Some("set SLUICE_HOME to the folder to use".to_string()),
        ),
        F::NeedsHome(_) => (
            "not checked, because the home is not there".to_string(),
            None,
        ),
        other => layout_or_lock(other).unwrap_or_else(|| access_key_or_space(other)),
    }
}

fn where_from(at: &Location) -> String {
    let from = if at.from_env {
        "from SLUICE_HOME"
    } else {
        "the default"
    };
    format!("{}, {from}", at.path.display())
}

fn layout_or_lock(finding: &Finding) -> Option<(String, Option<String>)> {
    use Finding as F;
    Some(match finding {
        F::LayoutCurrent(version) => (format!("version {version}"), None),
        F::LayoutOlder { found, current } => (
            format!("version {found}, older than the {current} this sluice writes"),
            Some(format!(
                "run `sluice id`, which migrates it to version {current}"
            )),
        ),
        F::LayoutNoFile(current) => (
            "no VERSION file".to_string(),
            Some(format!("run `sluice id`, which writes version {current}")),
        ),
        F::LayoutNewer { found, known } => (
            format!("version {found}, newer than the {known} this sluice reads"),
            Some(
                "use the newer sluice that wrote it, or set SLUICE_HOME to another folder"
                    .to_string(),
            ),
        ),
        F::LayoutUnreadable(why) => (
            why.clone(),
            Some(
                "make VERSION hold the layout version and a newline: 1 for a home this sluice made"
                    .to_string(),
            ),
        ),
        F::LockFree => ("free".to_string(), None),
        F::LockHeld(pid) => (format!("held by a running sluice{}", pid_text(*pid)), None),
        F::LockStale(pid) => (
            format!(
                "free, and lock.pid is left by a sluice{} that did not exit cleanly",
                pid_text(*pid)
            ),
            Some("run `sluice id`, which reclaims it".to_string()),
        ),
        F::LockUnreadable(why) => (
            format!("cannot be tried: {why}"),
            Some("check the permissions of the lock file in the home".to_string()),
        ),
        _ => return None,
    })
}

fn pid_text(pid: Option<u32>) -> String {
    pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default()
}

fn access_key_or_space(finding: &Finding) -> (String, Option<String>) {
    access_words(finding)
        .or_else(|| key_words(finding))
        .or_else(|| space_words(finding))
        .unwrap_or_default()
}

fn access_words(finding: &Finding) -> Option<(String, Option<String>)> {
    use Finding as F;
    Some(match finding {
        F::AccessPrivate => ("only this user can reach the home".to_string(), None),
        F::AccessUnreadable(why) => (
            format!("the home folder's access cannot be read: {why}"),
            Some("check the permissions of the home folder and the folders above it".to_string()),
        ),
        F::AccessOpenToOthers(mode) => (
            format!("the home has mode {mode:04o}, so other users can reach the key"),
            Some("run `chmod 700` on the home".to_string()),
        ),
        F::AccessOwnedByOther => (
            "another user owns the home, so they can change who can reach it".to_string(),
            Some(owned_by_other_fix().to_string()),
        ),
        F::AccessAclNotPrivate => (
            "the home's access list lets in more than this user, SYSTEM and Administrators"
                .to_string(),
            Some(
                "in the Properties of the home folder, under Security > Advanced, disable \
                 inheritance and remove every entry but yours, SYSTEM and Administrators"
                    .to_string(),
            ),
        ),
        _ => return None,
    })
}

fn key_words(finding: &Finding) -> Option<(String, Option<String>)> {
    use Finding as F;
    Some(match finding {
        F::KeyPrivate => ("32 bytes, readable only by this user".to_string(), None),
        F::KeyMissing => (
            "no identity key yet".to_string(),
            Some("run `sluice id`, which creates it".to_string()),
        ),
        F::KeyIsLink => (
            "identity.key is a link, not the key".to_string(),
            Some(
                "remove it, and run `sluice id` for a new identity if the key is gone".to_string(),
            ),
        ),
        F::KeyNotAFile => (
            "identity.key is not a regular file".to_string(),
            Some("move it elsewhere, and run `sluice id` for a new identity".to_string()),
        ),
        F::KeyHasLinks(links) => (
            format!("it has {links} links, and the identity key must have one"),
            Some(
                "remove the other names, or move it elsewhere and run `sluice id` for a new \
                 identity"
                    .to_string(),
            ),
        ),
        F::KeyOwnedByOther => (
            "another user owns it, who could change it whatever sluice sets".to_string(),
            Some(key_owned_by_other_fix().to_string()),
        ),
        F::KeyWrongSize(size) => (
            format!("{size} bytes, and an identity key is 32"),
            Some(
                "restore identity.key from a backup, or move it elsewhere and run `sluice id` \
                 for a new identity"
                    .to_string(),
            ),
        ),
        F::KeyOpenToOthers(mode) => (
            format!("mode {mode:04o}, so other users may be able to read it"),
            Some("run `chmod 600` on identity.key in the home".to_string()),
        ),
        F::KeyAclNotPrivate => (
            "its access list lets in more than this user, SYSTEM and Administrators".to_string(),
            Some(
                "in the Properties of identity.key, under Security > Advanced, disable \
                 inheritance and remove every entry but yours, SYSTEM and Administrators"
                    .to_string(),
            ),
        ),
        F::KeyUnreadable(why) => (
            format!("cannot be checked: {why}"),
            Some("check the permissions of identity.key in the home".to_string()),
        ),
        _ => return None,
    })
}

fn space_words(finding: &Finding) -> Option<(String, Option<String>)> {
    use Finding as F;
    Some(match finding {
        F::SpaceFree(free) => space(*free),
        F::SpaceUnreadable(why) => (
            format!("cannot be read: {why}"),
            Some("check that the home is on a mounted volume".to_string()),
        ),
        _ => return None,
    })
}

/// The fix for a home that another user owns, which only the platform's own tools can change.
fn owned_by_other_fix() -> &'static str {
    if cfg!(windows) {
        "take ownership: in the Properties of the home folder, under Security > Advanced, next \
         to Owner choose Change and pick your user, or point SLUICE_HOME at a folder of your own"
    } else {
        "run `chown` on the home to make this user its owner, or point SLUICE_HOME at a folder of \
         your own"
    }
}

/// The fix for a key that another user owns: the same tools, on the key itself.
fn key_owned_by_other_fix() -> &'static str {
    if cfg!(windows) {
        "take ownership of identity.key with `takeown`, set its access list with `icacls`, or \
         move it elsewhere and run `sluice id` for a new identity"
    } else {
        "run `chown` on identity.key to make this user its owner, or move it elsewhere and run \
         `sluice id` for a new identity"
    }
}

fn space(free: u64) -> (String, Option<String>) {
    let gib = free as f64 / f64::from(1u32 << 30);
    let detail = format!("{gib:.1} GiB free ({free} bytes)");
    if free < doctor::LOW_SPACE_BYTES {
        let fix =
            "free space on the home's volume: under 2 GiB, a transfer can run out".to_string();
        (detail, Some(fix))
    } else {
        (detail, None)
    }
}
