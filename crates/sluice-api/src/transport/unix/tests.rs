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
