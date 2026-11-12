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
async fn the_pipe_allows_this_user_system_and_administrators_and_nobody_else_when_read_back() {
    let name = r"\\.\pipe\sluice-control-dacl-test";
    let own = security::own_sid().unwrap();
    let sddl = security::private_sddl(&own).unwrap();
    let made = instance(name, &sddl, true).unwrap();
    let mut found = security::allowed_sids(&made).unwrap();
    found.sort();
    let mut expected = vec![
        security::sid_text(&own).unwrap(),
        "S-1-5-18".to_string(),
        "S-1-5-32-544".to_string(),
    ];
    expected.sort();
    assert_eq!(found, expected);
}

#[tokio::test]
async fn a_peer_the_check_refuses_gets_no_connection() {
    let dir = tempfile::tempdir().unwrap();
    let asked = std::sync::Arc::new(tokio::sync::Notify::new());
    let tell = asked.clone();
    let refuse: Check<NamedPipeServer> = std::sync::Arc::new(move |_| {
        tell.notify_one();
        Err(ApiError::Refused("another user".to_string()))
    });
    let (mut incoming, _guard, _) = listen_with(dir.path(), refuse).unwrap();
    let _client = open(&target(dir.path())).await.unwrap();
    asked.notified().await;
    let delivered = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        tokio_stream::StreamExt::next(&mut incoming.connections),
    )
    .await;
    assert!(
        delivered.is_err(),
        "a refused peer is never handed to the server"
    );
}

#[tokio::test]
async fn the_real_check_passes_a_client_of_this_user_by_impersonation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut incoming, _guard, _) = listen(dir.path()).unwrap();
    let _client = open(&target(dir.path())).await.unwrap();
    let got = tokio_stream::StreamExt::next(&mut incoming.connections).await;
    assert!(got.is_some_and(|c| c.is_ok()));
}

#[test]
fn the_name_of_the_pipe_depends_on_the_user_as_well_as_the_home() {
    let own = security::own_sid().unwrap();
    let home = Path::new(r"C:\Users\x\home-a");
    let mut hasher = blake3::Hasher::new();
    hasher.update(&own);
    hasher.update(r"c:\users\x\home-a".as_bytes());
    let with_sid = format!(
        r"\\.\pipe\sluice-control-{}",
        &hasher.finalize().to_hex().as_str()[..32]
    );
    assert_eq!(target(home).to_string_lossy(), with_sid);
    assert_ne!(
        target(home).to_string_lossy(),
        format!(
            r"\\.\pipe\sluice-control-{}",
            &blake3::hash(r"c:\users\x\home-a".as_bytes())
                .to_hex()
                .as_str()[..32]
        ),
        "the SID is part of the name"
    );
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
