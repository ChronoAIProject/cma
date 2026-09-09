use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "cma-restore-links-{}-{name}-{}",
            std::process::id(),
            stamp()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn restored_files_reuse_staging_storage_and_survive_staging_removal() {
    let fixture = Fixture::new("publish");
    let home = fixture.0.join("home");
    let staged = fixture.0.join("staged");
    let scratch = fixture.0.join("scratch");
    for path in [&home, &staged, &scratch] {
        std::fs::create_dir(path).unwrap();
    }
    std::fs::write(home.join("old"), "prior home").unwrap();
    std::fs::create_dir(staged.join("project")).unwrap();
    let source = staged.join("project/file");
    std::fs::write(&source, vec![23_u8; 1024 * 1024]).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink("file", staged.join("project/link")).unwrap();
    let source_metadata = std::fs::metadata(&source).unwrap();
    let home_inode = std::fs::metadata(&home).unwrap().ino();

    replace_home(&home, &staged, &scratch).unwrap();

    assert_eq!(std::fs::metadata(&home).unwrap().ino(), home_inode);
    let restored = home.join("project/file");
    let metadata = std::fs::metadata(&restored).unwrap();
    assert_eq!(metadata.dev(), source_metadata.dev());
    assert_eq!(metadata.ino(), source_metadata.ino());
    assert_eq!(metadata.nlink(), 2);
    assert_eq!(metadata.mode() & 0o777, 0o640);
    assert!(!home.join("old").exists());
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
    assert_eq!(
        std::fs::read_link(home.join("project/link")).unwrap(),
        Path::new("file")
    );
    std::fs::remove_dir_all(&staged).unwrap();
    assert_eq!(std::fs::metadata(&restored).unwrap().nlink(), 1);
    assert_eq!(std::fs::read(&restored).unwrap(), vec![23_u8; 1024 * 1024]);
    std::fs::write(&restored, "new user work").unwrap();
    assert_eq!(std::fs::read_to_string(restored).unwrap(), "new user work");
}

#[test]
fn failed_replacement_restores_original_inodes_and_removes_rollback_tree() {
    use std::os::unix::ffi::OsStrExt as _;

    let fixture = Fixture::new("rollback");
    let home = fixture.0.join("home");
    let staged = fixture.0.join("staged");
    let scratch = fixture.0.join("scratch");
    for path in [&home, &staged, &scratch] {
        std::fs::create_dir(path).unwrap();
    }
    let original = home.join("original");
    std::fs::write(&original, "retained user work").unwrap();
    let inode = std::fs::metadata(&original).unwrap().ino();
    std::fs::write(staged.join("partial"), "must roll back").unwrap();
    let fifo = std::ffi::CString::new(staged.join("unsupported").as_os_str().as_bytes()).unwrap();
    // Deliberately inject an unsupported staged entry after archive validation
    // to exercise rollback after the old home has already been removed.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let error = replace_home(&home, &staged, &scratch).unwrap_err();

    assert_eq!(error.code, "restore_entry_unsupported");
    assert_eq!(
        std::fs::read_to_string(&original).unwrap(),
        "retained user work"
    );
    assert_eq!(std::fs::metadata(&original).unwrap().ino(), inode);
    assert_eq!(std::fs::metadata(&original).unwrap().nlink(), 1);
    assert!(!home.join("partial").exists());
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn special_permissions_copy_without_modifying_the_source_inode() {
    let fixture = Fixture::new("permissions");
    let source = fixture.0.join("source");
    let destination = fixture.0.join("destination");
    std::fs::write(&source, "file payload").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o4640)).unwrap();
    let original = std::fs::metadata(&source).unwrap();

    copy_file(&source, &destination, 0o640).unwrap();

    assert_eq!(std::fs::metadata(&source).unwrap().mode() & 0o7777, 0o4640);
    let copied = std::fs::metadata(&destination).unwrap();
    assert_ne!(original.ino(), copied.ino());
    assert_eq!(copied.mode() & 0o7777, 0o640);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        std::fs::read(&destination).unwrap()
    );
}

#[test]
fn destination_collision_and_symlink_source_leave_existing_files_untouched() {
    let fixture = Fixture::new("collision");
    let source = fixture.0.join("source");
    let destination = fixture.0.join("destination");
    std::fs::write(&source, "source data").unwrap();
    std::fs::write(&destination, "existing destination").unwrap();
    assert!(copy_file(
        &source,
        &destination,
        mode(&std::fs::metadata(&source).unwrap())
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "existing destination"
    );
    let alias = fixture.0.join("alias");
    let absent = fixture.0.join("absent");
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    assert!(copy_file(&alias, &absent, 0o600).is_err());
    assert!(!absent.exists());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source data");
}
