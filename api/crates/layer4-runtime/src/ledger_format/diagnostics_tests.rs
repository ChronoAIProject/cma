use super::*;
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ledger-diagnostics-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        use std::io::Write;
        let path = self.0.join(name);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        path
    }
    fn config(&self) -> RuntimeConfig {
        let mut config = RuntimeConfig::for_test(self.0.join("home"));
        config.state_root = self.0.join("state");
        config.checkpoint_root = self.0.join("transfer/cma-checkpoints");
        config.home_identity = Some("a".repeat(43));
        config.process_env.insert(
            "TMPDIR".into(),
            self.0.join("transfer").to_str().unwrap().into(),
        );
        for path in [&config.home, &config.state_root, &self.0.join("transfer")] {
            std::fs::create_dir(path).unwrap();
        }
        config
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn expect_hold(operation: impl FnOnce() -> RuntimeError, predicate: &str, private_root: &Path) {
    let log = Log::default();
    let output = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer(move || output.clone())
        .finish();
    let error = tracing::subscriber::with_default(subscriber, operation);
    assert_eq!(error.code, "mutation_ledger_format_hold");
    assert!(!error.retryable);
    assert_eq!(
        error.detail,
        "runtime mutation ledger format requires verification"
    );
    let wire = error.response(layer3_adapter::wire::WireOperation::RuntimeStatus);
    let layer3_adapter::wire::ResponseFrame::Failure(failure) = wire else {
        panic!("ledger hold must retain its wire failure");
    };
    assert_eq!(failure.code, "mutation_ledger_format_hold");
    assert_eq!(
        failure.detail,
        "runtime mutation ledger format requires verification"
    );
    assert!(!failure.retryable);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains(&format!("predicate=\"{predicate}\"")),
        "{text}"
    );
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(!text.contains(private_root.to_str().unwrap()));
    assert!(!text.contains("private-content"));
}

#[test]
fn unsafe_metadata_still_holds_with_distinct_payload_free_predicates() {
    let fixture = Fixture::new();
    let file = fixture.file("private-name", b"private-content");
    assert_eq!(io::read(&file).unwrap(), b"private-content");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    expect_hold(
        || io::read(&file).unwrap_err(),
        "io.private_mode",
        &fixture.0,
    );
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = fixture.0.join("alias");
    std::fs::hard_link(&file, &alias).unwrap();
    expect_hold(
        || io::read(&file).unwrap_err(),
        "io.single_link",
        &fixture.0,
    );
    std::fs::remove_file(alias).unwrap();
    assert_eq!(io::read(&file).unwrap(), b"private-content");
    expect_hold(
        || io::lock(&file, false).unwrap_err(),
        "io.lock_empty",
        &fixture.0,
    );
    assert_eq!(std::fs::read(file).unwrap(), b"private-content");
}

#[test]
fn unsafe_ancestor_and_unavailable_io_never_include_the_path() {
    let fixture = Fixture::new();
    let real = fixture.0.join("real");
    std::fs::create_dir(&real).unwrap();
    let link = fixture.0.join("private-link-name");
    symlink(&real, &link).unwrap();
    expect_hold(
        || io::directories(&link).unwrap_err(),
        "io.ancestor_directory",
        &fixture.0,
    );
    expect_hold(
        || io::read(&fixture.0.join("missing-private-name")).unwrap_err(),
        "io.open",
        &fixture.0,
    );
}

#[test]
fn assignment_root_failures_are_distinguished_without_relaxing_the_contract() {
    let fixture = Fixture::new();
    let mut config = fixture.config();
    retained_transfer(&config).unwrap();
    let tmpdir = config.process_env.remove("TMPDIR").unwrap();
    expect_hold(
        || retained_transfer(&config).unwrap_err(),
        "transfer.tmpdir_missing",
        &fixture.0,
    );
    config.process_env.insert("TMPDIR".into(), tmpdir);
    config.checkpoint_root = fixture.0.join("other-private-root");
    expect_hold(
        || retained_transfer(&config).unwrap_err(),
        "transfer.assignment_roots",
        &fixture.0,
    );
}

#[test]
fn partial_v2_state_keeps_its_fail_closed_disposition_and_bytes() {
    let fixture = Fixture::new();
    let config = fixture.config();
    let staged = config.state_root.join(STAGE);
    std::fs::create_dir(&staged).unwrap();
    std::fs::write(staged.join("private-state"), b"private-content").unwrap();
    expect_hold(
        || match select(&config) {
            Ok(_) => panic!("partial publication must hold"),
            Err(error) => error,
        },
        "selection.partial_v2",
        &fixture.0,
    );
    assert_eq!(
        std::fs::read(staged.join("private-state")).unwrap(),
        b"private-content"
    );
    assert!(!config.state_root.join(MANIFEST).exists());
    assert!(!config.state_root.join("home-binding").exists());
}
