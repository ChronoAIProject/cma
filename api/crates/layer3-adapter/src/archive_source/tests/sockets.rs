use super::*;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{DirBuilderExt, FileTypeExt},
    net::UnixListener,
};

fn source(name: &[u8]) -> (Temp, SnapshotTree) {
    // Bind the fixture at a short address, then move the closed node to its
    // legal archived name. Neither capture nor restore connects to a service.
    let path = std::fs::canonicalize("/tmp")
        .unwrap()
        .join(format!("archive-{}", uuid::Uuid::new_v4().simple()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    let temp = Temp(path);
    let root = temp.0.join("source");
    std::fs::create_dir(&root).unwrap();
    let short = temp.0.join("s");
    drop(UnixListener::bind(&short).unwrap());
    let destination = root.join(std::ffi::OsStr::from_bytes(name));
    std::fs::rename(short, &destination).unwrap();
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).unwrap();
    let tree = SnapshotTree::open(&root).unwrap();
    (temp, tree)
}

fn roundtrip(name: &[u8], destination: &str) {
    let (temp, tree) = source(name);
    let manifest = serde_json::to_vec(&tree.entries).unwrap();
    let entries: Vec<Entry> = serde_json::from_slice(&manifest).unwrap();
    let socket = entries.iter().find(|e| e.kind == Kind::Socket).unwrap();
    assert_eq!(socket.path, vec![STANDARD.encode(name)]);
    assert_eq!(socket.metadata.mode & 0o170000, 0o140000);
    assert_eq!(socket.metadata.mode & 0o777, 0o600);
    assert_eq!(
        entries, tree.entries,
        "the manifest retains original metadata"
    );
    let mut restore = RestoreTree::create(&temp.0.join(destination)).unwrap();
    for entry in &entries {
        assert!(restore.add(entry).unwrap().is_none());
    }
    restore.finish().unwrap();
    tree.verify_unchanged().unwrap();
    let path = restore.path().join(std::ffi::OsStr::from_bytes(name));
    assert!(std::fs::symlink_metadata(path)
        .unwrap()
        .file_type()
        .is_socket());
    assert_eq!(std::fs::read_dir(restore.path()).unwrap().count(), 1);
    assert_eq!(serde_json::to_vec(&entries).unwrap(), manifest);
    assert!(RestoreTree::create(restore.path()).is_err());
}

#[test]
fn closed_socket_roundtrip_with_long_destination_prefix() {
    roundtrip(b"s", &"destination".repeat(16));
}

#[test]
fn closed_socket_roundtrip_with_long_leaf() {
    roundtrip(&[b's'; 120], "restore");
}

#[test]
fn closed_socket_roundtrip_with_multibyte_leaf() {
    let name = "界".repeat(40);
    assert_eq!(name.chars().count(), 40);
    assert_eq!(name.len(), 120);
    roundtrip(name.as_bytes(), "restore");
}

#[test]
#[cfg(target_os = "linux")]
fn closed_socket_roundtrip_with_non_utf8_leaf() {
    use std::os::unix::ffi::OsStringExt;
    let name = std::ffi::OsString::from_vec(vec![0xff; 120]);
    assert!(name.to_str().is_none());
    roundtrip(name.as_bytes(), "restore");
}

#[test]
#[cfg(target_os = "macos")]
fn filesystem_unsupported_name_refuses_without_losing_manifest_bytes() {
    let (temp, tree) = source(b"s");
    let mut socket = tree
        .entries
        .iter()
        .find(|e| e.kind == Kind::Socket)
        .unwrap()
        .clone();
    let name = vec![0xff; 120];
    socket.path = vec![STANDARD.encode(&name)];
    assert_eq!(decode(&socket.path[0]).unwrap(), name);
    let mut restore = RestoreTree::create(&temp.0.join("restore")).unwrap();
    restore.add(&tree.entries[0]).unwrap();
    assert!(restore.add(&socket).is_err());
    assert!(restore.finish().is_err());
    assert_eq!(std::fs::read_dir(restore.path()).unwrap().count(), 0);
    assert_eq!(decode(&socket.path[0]).unwrap(), name);
    tree.verify_unchanged().unwrap();
}

#[test]
fn socket_placement_never_overwrites_an_existing_node() {
    for kind in ["file", "socket", "symlink", "directory"] {
        let (temp, tree) = source(&[b's'; 120]);
        let socket = tree
            .entries
            .iter()
            .find(|e| e.kind == Kind::Socket)
            .unwrap();
        let mut restore = RestoreTree::create(&temp.0.join("restore")).unwrap();
        restore.add(&tree.entries[0]).unwrap();
        let target = restore.path().join("s".repeat(120));
        let protected = temp.0.join("protected");
        std::fs::write(&protected, b"keep").unwrap();
        match kind {
            "file" => std::fs::write(&target, b"original").unwrap(),
            "socket" => {
                let short = temp.0.join("s");
                drop(UnixListener::bind(&short).unwrap());
                std::fs::rename(short, &target).unwrap();
            }
            "symlink" => std::os::unix::fs::symlink(&protected, &target).unwrap(),
            _ => std::fs::create_dir(&target).unwrap(),
        }
        let before = SourceMetadata::from(&std::fs::symlink_metadata(&target).unwrap());
        assert!(restore.add(socket).is_err(), "existing {kind}");
        assert!(
            restore.finish().is_err(),
            "failed placement must poison verification"
        );
        assert_eq!(
            SourceMetadata::from(&std::fs::symlink_metadata(&target).unwrap()),
            before
        );
        assert_eq!(std::fs::read_dir(restore.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read(protected).unwrap(), b"keep");
        tree.verify_unchanged().unwrap();
    }
}

#[test]
fn final_census_rejects_a_missing_or_substituted_socket() {
    for substitute in [false, true] {
        let (temp, tree) = source(b"s");
        let mut restore = RestoreTree::create(&temp.0.join("restore")).unwrap();
        for entry in &tree.entries {
            restore.add(entry).unwrap();
        }
        std::fs::remove_file(restore.path().join("s")).unwrap();
        if substitute {
            std::fs::write(restore.path().join("s"), b"not a socket").unwrap();
        }
        assert!(restore.finish().is_err());
        tree.verify_unchanged().unwrap();
    }
}

#[test]
fn concurrent_socket_restores_leave_process_cwd_unchanged() {
    let (temp, tree) = source(&[b's'; 120]);
    let cwd = std::env::current_dir().unwrap();
    std::thread::scope(|scope| {
        for index in 0..8 {
            let path = temp.0.join(format!("restore-{index}"));
            let tree = &tree;
            let cwd = &cwd;
            scope.spawn(move || {
                let mut restore = RestoreTree::create(&path).unwrap();
                for entry in &tree.entries {
                    restore.add(entry).unwrap();
                    assert_eq!(&std::env::current_dir().unwrap(), cwd);
                }
                restore.finish().unwrap();
            });
        }
    });
    assert_eq!(std::env::current_dir().unwrap(), cwd);
    tree.verify_unchanged().unwrap();
}
