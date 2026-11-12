use super::*;

#[test]
fn a_peer_with_another_sid_is_refused_and_this_user_is_not() {
    let own = vec![
        1, 2, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 1, 0, 0, 0, 0xe9, 3, 0, 0,
    ];
    let mut other = own.clone();
    *other.last_mut().unwrap() = 1;
    other[16] = 0xea;
    assert!(check_sid(&own, &own).is_ok());
    assert!(matches!(check_sid(&other, &own), Err(ApiError::Refused(_))));
    assert!(
        matches!(check_sid(&[], &own), Err(ApiError::Refused(_))),
        "an unknown peer is refused"
    );
}

#[test]
fn the_access_list_names_this_user_system_and_administrators_and_nobody_else() {
    let own = security::own_sid().unwrap();
    let sddl = security::private_sddl(&own).unwrap();
    assert!(sddl.starts_with("D:P(A;;GA;;;S-1-5-"), "{sddl}");
    assert!(sddl.ends_with("(A;;GA;;;SY)(A;;GA;;;BA)"), "{sddl}");
    assert_eq!(sddl.matches("(A;").count(), 3, "{sddl}");
    assert!(
        !sddl.contains("WD") && !sddl.contains("AU"),
        "no Everyone or Authenticated Users"
    );
}

#[test]
fn two_homes_have_two_pipes_and_one_home_has_one() {
    let a = target(Path::new(r"C:\Users\x\home-a"));
    let b = target(Path::new(r"C:\Users\x\home-b"));
    assert_ne!(a, b);
    assert_eq!(a, target(Path::new(r"c:\users\x\HOME-A\")));
    assert!(a.to_string_lossy().starts_with(r"\\.\pipe\sluice-control-"));
}

#[tokio::test]
async fn a_client_of_the_same_user_connects_and_the_pipe_is_not_joined_by_a_second_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let (_incoming, _guard, shown) = listen(dir.path()).unwrap();
    let client = open(&target(dir.path())).await.expect("this user connects");
    drop(client);
    let again = listen(dir.path())
        .map(|_| ())
        .expect_err("a second daemon is refused");
    assert!(
        again.to_string().contains("cannot make the pipe"),
        "{again}"
    );
    assert!(shown.starts_with(r"\\.\pipe\"));
}
