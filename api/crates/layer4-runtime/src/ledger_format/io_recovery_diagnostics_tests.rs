use crate::config::RuntimeConfig;
use crate::ledger_format::{descriptor::Descriptor, publication, select, LOCK, PAGES, STAGE};
use crate::mutation_ledger::Ledger;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
    _writer: Option<std::fs::File>,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "ledger-recovery-diagnostics-{}",
            uuid::Uuid::new_v4()
        ));
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.state_root = root.join("state");
        config.checkpoint_root = root.join("transfer/cma-checkpoints");
        config.home_identity = Some("a".repeat(43));
        config.process_env.insert(
            "TMPDIR".into(),
            root.join("transfer").to_str().unwrap().into(),
        );
        for directory in [&config.home, &config.state_root, &root.join("transfer")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let writer = crate::workspace_removal_writer::claim(&config, "diagnostics").unwrap();
        crate::persistent_home::initialize(&config).unwrap();
        crate::workspace_removal_writer::publish_claim(&config, "diagnostics").unwrap();
        let outer = super::lock(&config.state_root.join(LOCK), true).unwrap();
        let stage = config.state_root.join(STAGE);
        let ledger = Ledger::open(stage.clone(), config.clone(), true, PAGES)
            .await
            .unwrap();
        let store = ledger.store_id().await.unwrap();
        ledger.close().await.unwrap();
        crate::fs_durability::sync_tree(&stage).unwrap();
        let descriptor = Descriptor::new(&config, store).unwrap();
        super::create(
            &config.state_root.join(publication::INTENT),
            &descriptor.bytes().unwrap(),
        )
        .unwrap();
        drop(outer);
        publication::inspect(&config).unwrap();
        Self {
            root,
            config,
            _writer: writer,
        }
    }

    fn snapshot(&self) -> Vec<Vec<u8>> {
        [
            self.config.state_root.join(publication::INTENT),
            self.config.state_root.join(STAGE).join("ledger.sqlite3"),
            self.config.state_root.join(STAGE).join("emergency.reserve"),
        ]
        .into_iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
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

fn expect_cause(fixture: &Fixture, fields: &[&str]) -> String {
    let log = Log::default();
    let output = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer(move || output.clone())
        .finish();
    let error =
        tracing::subscriber::with_default(subscriber, || select(&fixture.config).err().unwrap());
    let crate::failure::RuntimeError {
        code,
        detail,
        retryable,
    } = error;
    assert_eq!(code, "mutation_ledger_format_hold");
    assert_eq!(
        detail,
        "runtime mutation ledger format requires verification"
    );
    assert!(!retryable);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("predicate=\"publication.intent_database\""),
        "{text}"
    );
    for field in fields {
        assert!(text.contains(field), "missing {field}: {text}");
    }
    assert!(!text.contains(fixture.root.to_str().unwrap()), "{text}");
    assert!(!text.contains(&fixture.config.sandbox_id), "{text}");
    text
}

#[tokio::test]
async fn intent_recovery_preserves_exact_metadata_cause_and_existing_bytes() {
    for (name, role, predicate) in [
        ("", "store_directory", "storage.inspection.private_mode"),
        (
            "ledger.sqlite3",
            "database",
            "storage.inspection.private_mode",
        ),
        (
            "writer.lock",
            "writer_lock",
            "storage.inspection.private_mode",
        ),
        (
            "emergency.reserve",
            "emergency_reserve",
            "storage.file.private_mode",
        ),
    ] {
        let fixture = Fixture::new().await;
        let before = fixture.snapshot();
        let path = fixture.config.state_root.join(STAGE).join(name);
        let metadata = std::fs::metadata(&path).unwrap();
        let mode = if metadata.is_dir() { 0o750 } else { 0o640 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        expect_cause(
            &fixture,
            &[
                &format!("predicate=\"{predicate}\""),
                &format!("object_role=\"{role}\""),
                &format!("mode_octal={mode:04o}"),
                "required_group_other_bits=0",
            ],
        );
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, mode);
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), metadata.ino());
    }
}

#[tokio::test]
async fn intent_recovery_preserves_missing_reserve_errno_without_repair() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot();
    let reserve = fixture
        .config
        .state_root
        .join(STAGE)
        .join("emergency.reserve");
    let saved = fixture.root.join("saved-reserve");
    std::fs::rename(&reserve, &saved).unwrap();
    expect_cause(
        &fixture,
        &[
            "predicate=\"storage.file.open\"",
            "object_role=\"emergency_reserve\"",
            "operation=\"open_existing\"",
            "error_kind=NotFound",
            &format!("raw_os_error=Some({})", libc::ENOENT),
        ],
    );
    assert!(!reserve.exists());
    assert_eq!(std::fs::read(&saved).unwrap(), before[2]);
    std::fs::rename(saved, reserve).unwrap();
    assert_eq!(fixture.snapshot(), before);
    let writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.config.state_root.join(STAGE).join("writer.lock"))
        .unwrap();
    writer.try_lock().unwrap();
    let busy = expect_cause(
        &fixture,
        &[
            "predicate=\"storage.paths.writer_busy\"",
            "object_role=\"writer_lock\"",
        ],
    );
    assert!(!busy.contains("raw_os_error"), "{busy}");
    assert_eq!(fixture.snapshot(), before);
}
