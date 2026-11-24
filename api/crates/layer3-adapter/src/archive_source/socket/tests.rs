use super::*;
use crate::archive_source::tests::Temp;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};

#[test]
fn retained_parent_survives_path_replacement_without_writing_to_the_replacement() {
    let temp = Temp::new();
    let parent = temp.0.join("parent");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let handle = File::open(&parent).unwrap();
    let retained = temp.0.join("retained");
    std::fs::rename(&parent, &retained).unwrap();
    let outside = temp.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &parent).unwrap();
    restore(&handle, &[b's'; 120]).unwrap();
    assert!(std::fs::symlink_metadata(retained.join("s".repeat(120)))
        .unwrap()
        .file_type()
        .is_socket());
    assert_eq!(std::fs::read_dir(retained).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}

#[test]
fn bind_collision_is_refused_and_its_private_scratch_is_removed() {
    let temp = Temp::new();
    let parent = File::open(&temp.0).unwrap();
    assert!(reconstruct(&parent, b"result", |directory| {
        bind_closed(directory)?;
        bind_closed(directory)
    })
    .is_err());
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
}

#[test]
fn cleanup_failure_is_not_success_and_preserves_the_unexpected_entry() {
    let temp = Temp::new();
    let parent = File::open(&temp.0).unwrap();
    assert!(reconstruct(&parent, b"result", |directory| {
        bind_closed(directory)?;
        assert_eq!(
            unsafe { libc::mkdirat(directory.as_raw_fd(), c"unexpected".as_ptr(), 0o700) },
            0
        );
        Ok(())
    })
    .is_err());
    let entries = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    let scratch = entries.iter().find(|p| p.is_dir()).unwrap();
    assert!(scratch.join("unexpected").is_dir());
    assert!(!scratch.join("s").exists());
    assert!(std::fs::symlink_metadata(temp.0.join("result"))
        .unwrap()
        .file_type()
        .is_socket());
}

#[test]
fn unwinding_removes_the_scratch_and_closed_socket() {
    let temp = Temp::new();
    let parent = File::open(&temp.0).unwrap();
    let panic = std::panic::catch_unwind(|| {
        let _ = reconstruct(&parent, b"result", |directory| {
            bind_closed(directory)?;
            panic!("stop before publication");
        });
    });
    assert!(panic.is_err());
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
}

#[test]
fn non_private_parent_and_invalid_leaf_are_refused_before_helper_creation() {
    let temp = Temp::new();
    let parent = File::open(&temp.0).unwrap();
    for name in [&b""[..], b".", b"..", b"x/y", b"x\0y"] {
        assert!(restore(&parent, name).is_err());
    }
    std::fs::set_permissions(&temp.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(restore(&parent, b"s").is_err());
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
}
