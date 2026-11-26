use super::*;
use std::os::unix::fs::PermissionsExt;
mod publication;
mod sockets;
pub(super) struct Temp(pub(super) std::path::PathBuf);
impl Temp {
    pub(super) fn new() -> Self {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("retirement-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn full_tree_roundtrip_preserves_bytes_symlinks_and_hardlinks() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"conversation and ledger bytes").unwrap();
    std::fs::hard_link(source.join("data"), source.join("alias")).unwrap();
    std::os::unix::fs::symlink("../outside", source.join("link")).unwrap();
    let tree = SnapshotTree::open(&source).unwrap();
    let mut restore = RestoreTree::create(&temp.0.join("restored")).unwrap();
    for entry in &tree.entries {
        let mut output = restore.add(entry).unwrap();
        if entry.kind == Kind::File {
            let mut reader = tree.reader(entry).unwrap();
            let mut data = Vec::new();
            while let Some(chunk) = reader.chunk().unwrap() {
                if let Some(file) = output.as_mut() {
                    RestoreTree::write_chunk(file, &chunk).unwrap();
                }
                data.extend(chunk);
            }
            restore
                .verify_file(entry, &crate::permanent_custody::digest(&data))
                .unwrap();
        }
    }
    tree.verify_unchanged().unwrap();
    restore.finish().unwrap();
    assert_eq!(
        std::fs::metadata(restore.path().join("alias"))
            .unwrap()
            .ino(),
        std::fs::metadata(restore.path().join("data"))
            .unwrap()
            .ino()
    );
    assert_eq!(
        std::fs::read_link(restore.path().join("link")).unwrap(),
        Path::new("../outside")
    );
    assert!(RestoreTree::create(restore.path()).is_err());
}
#[test]
fn changed_source_and_path_substitution_fail_closed() {
    let temp = Temp::new();
    std::fs::write(temp.0.join("data"), b"first").unwrap();
    let tree = SnapshotTree::open(&temp.0).unwrap();
    let entry = tree.entries.iter().find(|e| e.kind == Kind::File).unwrap();
    let mut reader = tree.reader(entry).unwrap();
    std::fs::write(temp.0.join("data"), b"changed").unwrap();
    assert!(reader.chunk().is_err());
    assert!(tree.verify_unchanged().is_err());
    std::fs::remove_file(temp.0.join("data")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", temp.0.join("data")).unwrap();
    assert!(tree.reader(entry).is_err());
    assert!(decode(&STANDARD.encode(b"..")).is_err());
    assert!(decode(&STANDARD.encode(b"x/y")).is_err());
}
#[test]
fn private_inputs_reject_public_and_linked_files() {
    let temp = Temp::new();
    let path = temp.0.join("request");
    std::fs::write(&path, b"private").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_private_file(&path, 100).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_private_file(&path, 100).unwrap(), b"private");
    assert!(read_private_file(&path, 2).is_err());
    std::os::unix::fs::symlink(&path, temp.0.join("link")).unwrap();
    assert!(read_private_file(&temp.0.join("link"), 100).is_err());
}

#[test]
fn extended_metadata_is_retained_and_drift_detected() {
    use std::os::unix::ffi::OsStrExt;
    let temp = Temp::new();
    let file = temp.0.join("attributes");
    std::fs::write(&file, b"body").unwrap();
    let path = std::ffi::CString::new(file.as_os_str().as_bytes()).unwrap();
    let key = std::ffi::CString::new("user.cma.archive").unwrap();
    #[cfg(target_os = "linux")]
    let result =
        unsafe { libc::setxattr(path.as_ptr(), key.as_ptr(), b"first".as_ptr().cast(), 5, 0) };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            key.as_ptr(),
            b"first".as_ptr().cast(),
            5,
            0,
            0,
        )
    };
    assert_eq!(result, 0);
    let tree = SnapshotTree::open(&temp.0).unwrap();
    assert!(tree.entries[1].attributes.contains(&(
        STANDARD.encode(b"user.cma.archive"),
        STANDARD.encode(b"first")
    )));
    #[cfg(target_os = "linux")]
    let result =
        unsafe { libc::setxattr(path.as_ptr(), key.as_ptr(), b"other".as_ptr().cast(), 5, 0) };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            key.as_ptr(),
            b"other".as_ptr().cast(),
            5,
            0,
            0,
        )
    };
    assert_eq!(result, 0);
    assert!(tree.verify_unchanged().is_err());
}

#[test]
fn reconstruction_refuses_traversal_duplicate_entries_and_existing_symlink_parents() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data"), b"original").unwrap();
    let tree = SnapshotTree::open(&source).unwrap();
    let mut restored = RestoreTree::create(&temp.0.join("restore")).unwrap();
    restored.add(&tree.entries[0]).unwrap();
    assert!(restored.add(&tree.entries[0]).is_err());
    let mut bad = tree.entries[1].clone();
    bad.path = vec![STANDARD.encode(b".."), STANDARD.encode(b"escaped")];
    assert!(restored.add(&bad).is_err());
    let mut linked = tree.entries[1].clone();
    linked.path = vec![STANDARD.encode(b"link")];
    linked.kind = Kind::Symlink {
        target: STANDARD.encode(source.as_os_str().as_encoded_bytes()),
    };
    restored.add(&linked).unwrap();
    let mut child = tree.entries[1].clone();
    child.path = vec![STANDARD.encode(b"link"), STANDARD.encode(b"escaped")];
    assert!(restored.add(&child).is_err());
    assert!(!source.join("escaped").exists());
    assert_eq!(std::fs::read(source.join("data")).unwrap(), b"original");
}

#[test]
fn evidence_creation_refuses_symlink_shared_parents_and_changed_replays() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = Temp::new();
    let private = temp.0.join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    let evidence = private.join("receipt");
    publish_private_file(&evidence, b"original").unwrap();
    publish_private_file(&evidence, b"original").unwrap();
    assert_eq!(read_private_file(&evidence, 64).unwrap(), b"original");
    assert!(publish_private_file(&evidence, b"changed").is_err());
    assert_eq!(std::fs::metadata(&evidence).unwrap().mode() & 0o777, 0o600);
    let linked = temp.0.join("linked");
    symlink(&private, &linked).unwrap();
    assert!(publish_private_file(&linked.join("escaped"), b"refused").is_err());
    assert!(!private.join("escaped").exists());
    let alias = private.join("alias");
    symlink(&evidence, &alias).unwrap();
    assert!(publish_private_file(&alias, b"original").is_err());
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(publish_private_file(&private.join("shared"), b"refused").is_err());
    assert!(!private.join("shared").exists());
}
