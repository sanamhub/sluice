use super::*;

fn home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn a_peer_with_another_user_id_is_refused_and_this_user_is_not() {
    assert!(check_uid(1000, 1000).is_ok());
    assert!(matches!(check_uid(1001, 1000), Err(ApiError::Refused(_))));
    assert!(matches!(check_uid(0, 1000), Err(ApiError::Refused(_))));
}

#[tokio::test]
async fn the_socket_lives_in_a_folder_of_mode_0700_and_is_removed_when_the_daemon_stops() {
    let dir = home();
    let (incoming, guard, shown) = listen(dir.path()).unwrap();
    let folder = dir.path().join(FOLDER);
    assert_eq!(fs::metadata(&folder).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&shown).unwrap().mode() & 0o777, 0o600);
    drop(incoming);
    drop(guard);
    assert!(!Path::new(&shown).exists());
}

#[tokio::test]
async fn a_stale_socket_file_is_taken_over() {
    let dir = home();
    let path = PathBuf::from(target(dir.path()));
    fs::create_dir(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    // A listener that is dropped leaves its file behind, as a daemon that was killed does.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    let (_incoming, _guard, _) = listen(dir.path()).expect("the stale socket is taken over");
    assert!(
        UnixStream::connect(&path).await.is_ok(),
        "the new daemon answers on the path"
    );
}

#[tokio::test]
async fn a_socket_a_live_daemon_answers_on_is_not_taken_over() {
    let dir = home();
    let (_incoming, _guard, _) = listen(dir.path()).unwrap();
    let again = listen(dir.path())
        .map(|_| ())
        .expect_err("the second daemon is refused");
    assert!(again.to_string().contains("already listening"), "{again}");
}

#[test]
fn a_socket_path_that_is_a_link_is_refused_and_what_it_names_is_left_alone() {
    let dir = home();
    let path = PathBuf::from(target(dir.path()));
    fs::create_dir(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    fs::write(&elsewhere, b"keep").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    let refused = listen(dir.path())
        .map(|_| ())
        .expect_err("a link is refused");
    assert!(refused.to_string().contains("link"), "{refused}");
    assert_eq!(fs::read(&elsewhere).unwrap(), b"keep");
}

#[test]
fn a_file_that_is_not_a_socket_is_refused_and_kept() {
    let dir = home();
    let path = PathBuf::from(target(dir.path()));
    fs::create_dir(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, b"mine").unwrap();
    assert!(listen(dir.path()).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"mine");
}

#[test]
fn a_folder_that_others_can_reach_is_refused() {
    let dir = home();
    let folder = dir.path().join(FOLDER);
    fs::create_dir(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    let refused = listen(dir.path())
        .map(|_| ())
        .expect_err("an open folder is refused");
    assert!(refused.to_string().contains("0700"), "{refused}");
}

#[tokio::test]
async fn a_peer_the_check_refuses_gets_no_connection() {
    let dir = home();
    let asked = std::sync::Arc::new(tokio::sync::Notify::new());
    let tell = asked.clone();
    let refuse: Check<UnixStream> = std::sync::Arc::new(move |_| {
        tell.notify_one();
        Err(ApiError::Refused("another user".to_string()))
    });
    let (mut incoming, _guard, shown) = listen_with(dir.path(), refuse).unwrap();
    let _client = UnixStream::connect(&shown).await.unwrap();
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
async fn the_real_check_passes_a_peer_of_this_user() {
    let dir = home();
    let (mut incoming, _guard, shown) = listen(dir.path()).unwrap();
    let _client = UnixStream::connect(&shown).await.unwrap();
    let got = tokio_stream::StreamExt::next(&mut incoming.connections).await;
    assert!(got.is_some_and(|c| c.is_ok()));
}

#[test]
fn a_run_folder_another_user_could_reach_or_a_link_is_refused_to_the_client_too() {
    let dir = home();
    let folder = dir.path().join(FOLDER);
    fs::create_dir(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
    check_folder(&folder).unwrap();
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(check_folder(&folder).is_err());
    let link = dir.path().join("run-link");
    std::os::unix::fs::symlink(&folder, &link).unwrap();
    assert!(check_folder(&link).is_err());
}

#[test]
fn the_socket_of_a_home_that_is_a_link_is_in_the_folder_the_link_leads_to() {
    let dir = home();
    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(target(&link), target(&real));
}
