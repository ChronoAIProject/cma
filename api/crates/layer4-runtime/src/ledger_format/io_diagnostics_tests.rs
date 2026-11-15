use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ledger-io-cause-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn file(&self, mode: u32) -> PathBuf {
        let path = self.0.join("private-name");
        create(&path, b"private-content").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_hold(operation: impl FnOnce() -> RuntimeError, fields: &[&str]) -> String {
    capture_failure(
        operation,
        fields,
        "mutation_ledger_format_hold",
        "runtime mutation ledger format requires verification",
    )
}

fn capture_failure(
    operation: impl FnOnce() -> RuntimeError,
    fields: &[&str],
    code: &str,
    detail: &str,
) -> String {
    let log = Log::default();
    let output = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer(move || output.clone())
        .finish();
    let error = tracing::subscriber::with_default(subscriber, operation);
    assert_eq!(error.code, code);
    assert_eq!(error.detail, detail);
    assert!(!error.retryable);
    let layer3_adapter::wire::ResponseFrame::Failure(wire) =
        error.response(layer3_adapter::wire::WireOperation::RuntimeStatus)
    else {
        panic!("format hold must retain its wire failure");
    };
    assert_eq!(wire.code, code);
    assert_eq!(wire.detail, detail);
    assert!(!wire.retryable);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    for field in fields {
        assert!(text.contains(field), "missing {field}: {text}");
    }
    assert!(!text.contains("private-name"), "{text}");
    assert!(!text.contains("private-content"), "{text}");
    text
}

#[test]
fn retained_mode_failure_reports_observed_mode_and_read_or_lock_without_repair() {
    let fixture = Fixture::new();
    let path = fixture.file(0o640);
    let inode = std::fs::metadata(&path).unwrap().ino();
    for operation in ["read_existing", "lock_existing"] {
        let text = capture_hold(
            || match operation {
                "read_existing" => read(&path).unwrap_err(),
                _ => lock(&path, false).unwrap_err(),
            },
            &[
                "predicate=\"io.private_mode\"",
                operation,
                "mode_octal=0640",
                "group_other_bits=32",
                "required_group_other_bits=0",
                "owner_read=true",
                "effective_owner=true",
            ],
        );
        assert!(!text.contains(fixture.0.to_str().unwrap()));
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o640);
        assert_eq!(metadata.ino(), inode);
        assert_eq!(std::fs::read(&path).unwrap(), b"private-content");
    }
}

#[test]
fn exclusive_creation_reports_existing_file_errno_without_overwriting() {
    let fixture = Fixture::new();
    let path = fixture.file(0o600);
    for operation in ["exclusive_create", "exclusive_lock"] {
        let text = capture_hold(
            || match operation {
                "exclusive_create" => create(&path, b"replacement").unwrap_err(),
                _ => lock(&path, true).unwrap_err(),
            },
            &[
                "predicate=\"io.open\"",
                operation,
                "error_kind=AlreadyExists",
                &format!("raw_os_error=Some({})", libc::EEXIST),
            ],
        );
        assert!(!text.contains(fixture.0.to_str().unwrap()));
        assert_eq!(std::fs::read(&path).unwrap(), b"private-content");
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
    }
}

#[test]
fn missing_retained_file_reports_read_operation_and_original_errno() {
    let fixture = Fixture::new();
    let text = capture_hold(
        || read(&fixture.0.join("private-name")).unwrap_err(),
        &[
            "predicate=\"io.open\"",
            "operation=\"read_existing\"",
            "error_kind=NotFound",
            &format!("raw_os_error=Some({})", libc::ENOENT),
        ],
    );
    assert!(!text.contains(fixture.0.to_str().unwrap()));
}

#[test]
fn syscall_cause_logs_kind_and_errno_but_never_error_payload() {
    capture_hold(
        || {
            io_error(
                "io.read",
                "read_existing",
                &std::io::Error::new(std::io::ErrorKind::PermissionDenied, "private-content"),
            )
        },
        &["error_kind=PermissionDenied", "raw_os_error=None"],
    );
    capture_hold(
        || {
            io_error(
                "io.file_sync",
                "exclusive_create",
                &std::io::Error::from_raw_os_error(libc::EIO),
            )
        },
        &[
            "predicate=\"io.file_sync\"",
            &format!("raw_os_error=Some({})", libc::EIO),
        ],
    );
    capture_failure(
        || {
            lock_error(
                std::fs::TryLockError::Error(std::io::Error::from_raw_os_error(libc::EIO)),
                "lock_existing",
            )
        },
        &[
            "predicate=\"io.writer_lock_syscall\"",
            &format!("raw_os_error=Some({})", libc::EIO),
        ],
        "mutation_ledger_writer_held",
        "runtime mutation ledger has another writer",
    );
    let busy = capture_failure(
        || lock_error(std::fs::TryLockError::WouldBlock, "lock_existing"),
        &["predicate=\"io.writer_lock_busy\""],
        "mutation_ledger_writer_held",
        "runtime mutation ledger has another writer",
    );
    assert!(!busy.contains("raw_os_error"), "{busy}");
}

#[test]
fn link_refusal_is_distinct_from_mode_refusal_and_preserves_both_names() {
    let fixture = Fixture::new();
    let path = fixture.file(0o600);
    let alias = fixture.0.join("alias");
    std::fs::hard_link(&path, &alias).unwrap();
    capture_hold(
        || read(&path).unwrap_err(),
        &[
            "predicate=\"io.single_link\"",
            "single_link=false",
            "mode_octal=0600",
            "group_other_bits=0",
        ],
    );
    assert_eq!(std::fs::read(path).unwrap(), b"private-content");
    assert_eq!(std::fs::read(alias).unwrap(), b"private-content");
}

#[test]
fn accepted_owner_only_modes_are_not_normalized_by_diagnostics() {
    let fixture = Fixture::new();
    let path = fixture.file(0o700);
    assert_eq!(read(&path).unwrap(), b"private-content");
    assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o7777, 0o700);
}
