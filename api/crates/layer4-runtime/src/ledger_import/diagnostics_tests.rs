use super::*;
use crate::ledger_import::{invalid, invalid_guard, invalid_io, io::Directory};

fn panic_message(operation: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let panic = std::panic::catch_unwind(operation).unwrap_err();
    *panic.downcast::<String>().unwrap()
}

#[test]
fn actual_openat_failure_keeps_errno_and_guard_in_assertion_only() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let root = std::env::temp_dir().join(format!("ledger-diagnostic-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let root = Temp(root.canonicalize().unwrap());
    let link = root.0.join("private-link-name");
    std::os::unix::fs::symlink(&root.0, &link).unwrap();
    let directory = Directory::open(&root.0).unwrap();
    let message = panic_message(|| {
        assert_ok(|| directory.open_at("private-link-name", 0));
    });
    assert!(message.contains("io.open_at.openat"), "{message}");
    assert!(
        message.contains(&format!("Some({})", libc::ELOOP)),
        "{message}"
    );
    assert!(message.contains("io.rs:"), "{message}");
    assert!(!message.contains("private-link-name"));
    assert!(!message.contains(root.0.to_str().unwrap()));
    let error = directory.open_at("private-link-name", 0).err().unwrap();
    assert_eq!(error.code, "mutation_ledger_migration_invalid");
    assert_eq!(
        error.detail,
        "runtime mutation ledger migration input requires verification"
    );
    assert!(!error.retryable);
}

#[test]
fn logical_refusal_does_not_invent_an_os_error() {
    let message = panic_message(|| {
        assert_ok(|| Directory::open(std::path::Path::new("relative")));
    });
    assert!(message.contains("io.open.absolute_path"), "{message}");
    assert!(message.contains("None"), "{message}");
    assert_eq!(assert_ok(|| Ok(7)), 7);
    assert!(capture(|| Ok(())).1.is_none());
}

#[test]
fn first_failure_is_thread_scoped_and_restored_after_nested_panic() {
    let (_, first) = capture(|| {
        let error = invalid_io("first", std::io::Error::from_raw_os_error(libc::ENOENT));
        std::thread::spawn(|| {
            let (_, own) = capture::<()>(|| Err(invalid_guard("other_thread")));
            assert_eq!(own.unwrap().guard, "other_thread");
        })
        .join()
        .unwrap();
        let _ = panic_message(|| assert_ok::<()>(|| Err(invalid_guard("nested"))));
        let _ = invalid_guard("cleanup");
        Err::<(), _>(error)
    });
    let first = first.unwrap();
    assert_eq!(first.guard, "first");
    assert_eq!(first.os_error.unwrap().raw_os_error(), Some(libc::ENOENT));
    assert!(capture(|| Ok(())).1.is_none());
}

#[test]
fn unnamed_semantic_guard_retains_exact_source_location() {
    let line = line!() + 1;
    let (_, rejection) = capture::<()>(|| Err(invalid()));
    let rejection = rejection.unwrap();
    assert_eq!(rejection.guard, "invalid");
    assert_eq!(rejection.site.line(), line);
    assert!(rejection.os_error.is_none());
}
