//! Trustees that Windows writes back as two-letter aliases. The built-in Administrator is `LA`
//! for the machine's own RID-500 account, `SYSTEM` is `SY`, and `Administrators` is `BA`, so a list
//! built from full SIDs has to be compared after the same conversion that reads it back.

use super::super::admin::built_in_administrator_sid;
use super::super::lists::has_private_list_for;
use super::super::{INHERIT, canonical_sddl, private_sddl};

/// The SID of this machine's built-in Administrator, which every Windows has even when the account
/// is disabled. It is the machine's account domain with RID 500, read from the local security
/// authority, so a domain account or a Microsoft Entra account does not change it.
fn built_in_administrator() -> String {
    built_in_administrator_sid().expect("the machine's account domain and its RID 500")
}

/// A list that is protected from inheritance, as one sluice set on a home is.
fn protected(sddl: &str) -> String {
    sddl.replacen("D:", "D:P", 1)
}

/// The conversion shortens the machine's own built-in Administrator, which is the trustee that
/// made the two texts differ.
#[test]
fn the_round_trip_shortens_the_built_in_administrator_to_la() {
    let admin = built_in_administrator();
    let canonical = canonical_sddl(&protected(&private_sddl(&admin, INHERIT))).unwrap();
    assert!(canonical.contains(";LA)"), "{canonical}");
}

/// The cause of the refusal: a wanted list for the built-in Administrator is private when the list
/// read back says `LA`, because both name the same account.
#[test]
fn a_list_for_the_built_in_administrator_is_private_when_it_reads_back_as_la() {
    let admin = built_in_administrator();
    let found = canonical_sddl(&protected(&private_sddl(&admin, INHERIT))).unwrap();
    assert!(found.contains(";LA)"), "{found}");
    assert!(has_private_list_for(&found, INHERIT, &admin).unwrap());
}

/// `SYSTEM` has an alias too, so a list that names `S-1-5-18` and one that says `SY` are the same.
#[test]
fn a_list_for_local_system_is_private_when_it_reads_back_as_sy() {
    let wanted = protected(&private_sddl("S-1-5-18", INHERIT));
    let found = canonical_sddl(&wanted).unwrap();
    assert!(found.contains(";SY)"), "{found}");
    assert!(has_private_list_for(&found, INHERIT, "S-1-5-18").unwrap());
}

/// The alias is not a wildcard: an extra entry is still another entry, even beside `LA`.
#[test]
fn an_extra_entry_is_not_private_even_with_an_alias() {
    let admin = built_in_administrator();
    let wanted = protected(&private_sddl(&admin, INHERIT));
    let mut found = canonical_sddl(&wanted).unwrap();
    assert!(found.contains(";LA)"), "{found}");
    found.push_str(&format!("(A;{INHERIT};FR;;;WD)"));
    assert!(!has_private_list_for(&found, INHERIT, &admin).unwrap());
}

/// The same for an entry that is missing: `LA` does not stand in for the user who should be there.
#[test]
fn a_missing_entry_is_not_private_even_with_an_alias() {
    let admin = built_in_administrator();
    let found = canonical_sddl(&format!(
        "D:P(A;{INHERIT};FA;;;{admin})(A;{INHERIT};FA;;;SY)"
    ))
    .unwrap();
    assert!(found.contains(";LA)"), "{found}");
    assert!(!has_private_list_for(&found, INHERIT, &admin).unwrap());
}

/// Only the machine's own RID-500 account is `LA`. A SID from another account domain stays in its
/// full form, so the test above uses the machine's account and not a made-up domain.
#[test]
fn a_sid_from_another_account_domain_keeps_its_full_form() {
    let other = "S-1-5-21-1-2-3-500";
    let canonical = canonical_sddl(&protected(&private_sddl(other, INHERIT))).unwrap();
    assert!(canonical.contains(other), "{canonical}");
}
