use super::super::*;
use crate::proto::RootPath;
use crate::roots::NO_ROOTS;

fn service(roots: &[std::path::PathBuf]) -> (tempfile::TempDir, Service) {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(&dir.path().join("home")).unwrap();
    let roots = Roots::open(roots).unwrap();
    (dir, Service::new(home, roots, RelayModeChoice::Disabled))
}

fn at(root: &std::path::Path, path: &str) -> Option<RootPath> {
    Some(RootPath {
        root: root.to_str().unwrap().to_string(),
        path: path.to_string(),
    })
}

fn listed(response: &proto::ListEntriesResponse) -> Vec<(String, i32, u64)> {
    response
        .entries
        .iter()
        .map(|e| (e.name.clone().unwrap().text, e.kind, e.size))
        .collect()
}

#[tokio::test]
async fn the_roots_are_listed_by_label_and_never_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let (_home, service) = service(std::slice::from_ref(&root));
    let answer = service.list_roots();
    assert_eq!(
        answer.roots,
        vec![proto::RootInfo {
            name: "files".to_string()
        }]
    );
    let by_label = RootPath {
        root: "files".to_string(),
        path: String::new(),
    };
    assert!(
        service
            .list_entries(proto::ListEntriesRequest {
                path: Some(by_label)
            })
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_folder_is_listed_with_folders_first_and_a_file_is_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("a.txt"), b"abc").unwrap();
    let (_home, service) = service(std::slice::from_ref(&root));
    let answer = service
        .list_entries(proto::ListEntriesRequest {
            path: at(&root, ""),
        })
        .await
        .unwrap();
    assert!(!answer.truncated);
    assert_eq!(
        listed(&answer),
        vec![
            ("docs".to_string(), proto::EntryKind::Folder as i32, 0),
            ("a.txt".to_string(), proto::EntryKind::File as i32, 3),
        ]
    );
    let file = service
        .list_entries(proto::ListEntriesRequest {
            path: at(&root, "a.txt"),
        })
        .await
        .unwrap_err();
    assert!(matches!(file, ApiError::Invalid(_)), "{file:?}");
}

#[tokio::test]
async fn a_listing_follows_the_rules_of_every_path_under_a_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(dir.path().join("secret"), b"x").unwrap();
    let (_home, service) = service(std::slice::from_ref(&root));
    for path in ["..", "../..", "/", "missing"] {
        let refused = service
            .list_entries(proto::ListEntriesRequest {
                path: at(&root, path),
            })
            .await;
        assert!(refused.is_err(), "{path} was listed");
    }
    let unknown = service
        .list_entries(proto::ListEntriesRequest {
            path: at(dir.path(), ""),
        })
        .await;
    assert!(unknown.is_err(), "a folder that is not a root was listed");
    let (_other, none) = self::service(&[]);
    let refused = none
        .list_entries(proto::ListEntriesRequest { path: None })
        .await
        .unwrap_err();
    assert_eq!(refused, ApiError::Refused(NO_ROOTS.to_string()));
}

#[tokio::test]
async fn a_folder_with_more_entries_than_the_limit_is_cut_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let count = super::MAX_ENTRIES + 5;
    for n in 0..count {
        std::fs::File::create(root.join(format!("f{n:05}"))).unwrap();
    }
    let (_home, service) = service(std::slice::from_ref(&root));
    let answer = service
        .list_entries(proto::ListEntriesRequest {
            path: at(&root, ""),
        })
        .await
        .unwrap();
    assert_eq!(answer.entries.len(), super::MAX_ENTRIES);
    assert!(answer.truncated);
}
