use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(super) async fn interrupted(step: &str) -> Fixture {
    let fixture = Fixture::new();
    set_failure(&fixture.config, step);
    assert!(fixture.open().await.is_err(), "{step}");
    set_failure(&fixture.config, "");
    let intent = fixture.config.state_root.join(publication::INTENT);
    if intent.exists() {
        std::fs::remove_file(intent).unwrap(); // Reconstruct deployed7a's no-intent prefix.
    }
    fixture
}

pub(super) fn bytes(fixture: &Fixture) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &std::path::Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !path.exists() {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                visit(&entry.path(), result);
            } else if kind.is_file() {
                result.insert(entry.path(), std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(&fixture.root, &mut result);
    result
}

fn create(path: &std::path::Path, content: &[u8]) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(content)
        .unwrap();
}

#[tokio::test]
async fn empty_database_creation_prefixes_restart_as_fresh() {
    for prefix in 0..5 {
        let fixture = interrupted("locked").await;
        let stage = fixture.config.state_root.join(STAGE);
        if prefix > 0 {
            std::fs::create_dir(&stage).unwrap();
            std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        if prefix > 1 {
            create(&stage.join("writer.lock"), b"");
        }
        if prefix > 2 {
            create(&stage.join("ledger.sqlite3"), b"");
        }
        if prefix > 3 {
            create(&stage.join("emergency.reserve"), &[0xa5; 1024]);
        }
        let ledger = fixture.open().await.unwrap().unwrap();
        assert!(fixture.config.state_root.join(MANIFEST).exists());
        assert!(!stage.exists());
        close(ledger).await;
    }
}

#[tokio::test]
async fn every_no_intent_staged_barrier_prefix_discards_only_the_unused_store() {
    for step in [
        "closed",
        "barrier-0",
        "barrier-1",
        "barrier-2",
        "barrier-3",
        "barrier-4",
    ] {
        let fixture = interrupted(step).await;
        let database = fixture.config.state_root.join(STAGE).join("ledger.sqlite3");
        let db = rusqlite::Connection::open(&database).unwrap();
        let old: Vec<u8> = db
            .query_row("SELECT store_id FROM metadata", [], |row| row.get(0))
            .unwrap();
        db.close().unwrap();
        let ledger = fixture.open().await.unwrap().unwrap();
        let new = ledger.store_id().await.unwrap();
        assert_ne!(
            uuid::Uuid::parse_str(&new).unwrap().as_bytes().as_slice(),
            old.as_slice()
        );
        close(ledger).await;
        let ledger = fixture.open().await.unwrap().unwrap();
        assert_eq!(ledger.store_id().await.unwrap(), new);
        close(ledger).await;
    }
}

#[tokio::test]
async fn interrupted_discard_restarts_at_every_durable_removal() {
    for step in (0..8)
        .map(|index| format!("discard-{index}"))
        .chain(["discard-stage".into()])
    {
        let fixture = interrupted("barriers").await;
        set_failure(&fixture.config, &step);
        assert!(fixture.open().await.is_err(), "{step}");
        set_failure(&fixture.config, "");
        let ledger = fixture.open().await.unwrap().unwrap();
        assert!(fixture.config.state_root.join(MANIFEST).exists());
        close(ledger).await;
    }
}

#[tokio::test]
async fn staged_effect_schema_accounting_and_identity_residue_never_get_discarded() {
    for change in [
        "UPDATE accounting SET revision=1",
        "UPDATE accounting SET reserved=1",
        "UPDATE accounting SET operations=1",
        "UPDATE accounting SET authority=1",
        "UPDATE accounting SET terminal=1",
        "UPDATE metadata SET generation=x'0000000000000002'",
        "UPDATE metadata SET home='foreign'",
        "UPDATE metadata SET admission='import_held'",
        "DELETE FROM metadata", "DELETE FROM accounting", "DROP TABLE cleanup_budget",
        "INSERT INTO authority VALUES(x'0000000000000001','resource',x'0000000000000001',x'01',NULL,NULL)",
        "INSERT INTO scope_budget VALUES(x'0000000000000001',0,0)",
        "INSERT INTO cleanup_budget VALUES(x'0000000000000001','resource',1)",
        "INSERT INTO operations VALUES(x'0000000000000001','operation',x'01',0,1,NULL)",
        "INSERT INTO logical_processes VALUES(x'0000000000000001','agent','/cwd','/home')",
    ] {
        let fixture = interrupted("closed").await;
        let db = rusqlite::Connection::open(fixture.config.state_root.join(STAGE).join("ledger.sqlite3")).unwrap();
        db.execute_batch(change).unwrap(); db.close().unwrap();
        let before = bytes(&fixture);
        assert!(fixture.open().await.is_err(), "{change}");
        assert_eq!(bytes(&fixture), before, "{change}");
    }
}

#[tokio::test]
async fn unknown_unsafe_installation_and_retirement_state_is_preserved() {
    for kind in [
        "unknown",
        "install",
        "journal",
        "retirement",
        "mode",
        "link",
        "symlink",
        "reserve",
        "binding",
        "generation",
    ] {
        let mut fixture = interrupted("closed").await;
        let stage = fixture.config.state_root.join(STAGE);
        match kind {
            "unknown" => create(&fixture.config.state_root.join("unknown"), b"retained"),
            "install" => {
                std::fs::create_dir_all(fixture.config.checkpoint_root.join("runtime-state"))
                    .unwrap();
                create(
                    &fixture
                        .config
                        .checkpoint_root
                        .join("runtime-state/install-v2.json"),
                    b"retained",
                );
            }
            "journal" => create(&stage.join("ledger.sqlite3-journal"), b"retained"),
            "retirement" => create(
                &fixture
                    .config
                    .state_root
                    .join("mutation-retirement-v1.current.json"),
                b"retained",
            ),
            "mode" => std::fs::set_permissions(
                stage.join("ledger.sqlite3"),
                std::fs::Permissions::from_mode(0o640),
            )
            .unwrap(),
            "link" => std::fs::hard_link(
                stage.join("ledger.sqlite3"),
                fixture.root.join("retained-link"),
            )
            .unwrap(),
            "symlink" => {
                let retained = fixture.root.join("retained-database");
                std::fs::rename(stage.join("ledger.sqlite3"), &retained).unwrap();
                std::os::unix::fs::symlink(&retained, stage.join("ledger.sqlite3")).unwrap();
            }
            "reserve" => std::fs::write(stage.join("emergency.reserve"), b"retained").unwrap(),
            "binding" => {
                let path = fixture.config.state_root.join("home-binding/binding.json");
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["restore"] =
                    serde_json::json!({"request_sha256":"a".repeat(64),"completed":true});
                std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "generation" => fixture.config.generation = 2,
            _ => unreachable!(),
        }
        let before = bytes(&fixture);
        assert!(fixture.open().await.is_err(), "{kind}");
        assert_eq!(bytes(&fixture), before, "{kind}");
    }
}

pub(super) fn replace_path(config: &RuntimeConfig) {
    if FAIL
        .lock()
        .unwrap()
        .get(&config.state_root)
        .is_some_and(|v| v == "replace-aborted")
    {
        let stage = config.state_root.join(STAGE);
        std::fs::rename(&stage, config.state_root.join("retained-original")).unwrap();
        std::fs::create_dir(&stage).unwrap();
        create(&stage.join("foreign"), b"retain-foreign");
    }
}

#[tokio::test]
async fn replaced_staged_database_stays_untouched_after_read_only_proof() {
    let fixture = interrupted("barriers").await;
    let original =
        std::fs::read(fixture.config.state_root.join(STAGE).join("ledger.sqlite3")).unwrap();
    set_failure(&fixture.config, "replace-aborted");
    assert!(fixture.open().await.is_err());
    set_failure(&fixture.config, "");
    assert_eq!(
        std::fs::read(
            fixture
                .config
                .state_root
                .join("retained-original/ledger.sqlite3")
        )
        .unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(fixture.config.state_root.join(STAGE).join("foreign")).unwrap(),
        b"retain-foreign"
    );
    assert!(!fixture.config.state_root.join(MANIFEST).exists());
}

#[tokio::test]
async fn interrupted_initial_supervisor_restarts_and_reports_durable_installation() {
    let fixture = Fixture::new();
    set_failure(&fixture.config, "closed");
    assert!(crate::supervisor::Supervisor::start(
        fixture.config.clone(),
        crate::writer_fence::SupervisorBoundary::default()
    )
    .await
    .is_err());
    set_failure(&fixture.config, "");
    assert!(fixture.config.state_root.join(STAGE).exists());
    assert!(!fixture.config.state_root.join(publication::INTENT).exists());
    let supervisor = crate::supervisor::Supervisor::start(
        fixture.config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .await
    .unwrap();
    let response = supervisor
        .dispatch(layer3_adapter::wire::RequestFrame::Install(
            layer3_adapter::InstallRequest::new("recovered-initial-install", vec![]),
        ))
        .await;
    assert!(
        matches!(response, layer3_adapter::wire::ResponseFrame::Install(_)),
        "{response:?}"
    );
    let status = supervisor
        .dispatch(layer3_adapter::wire::RequestFrame::RuntimeStatus(
            layer3_adapter::RuntimeStatusRequest {
                sandbox_id: fixture.config.sandbox_id.clone(),
                generation: fixture.config.generation,
                home_identity: fixture.config.home_identity.clone().unwrap(),
            },
        ))
        .await;
    assert!(
        matches!(status,layer3_adapter::wire::ResponseFrame::RuntimeStatus(ref value) if value.installation_ready),
        "{status:?}"
    );
    assert!(fixture.config.state_root.join(MANIFEST).exists());
    assert!(!fixture.config.state_root.join(STAGE).exists());
}

#[tokio::test]
async fn empty_prefix_private_mode_refusal_logs_cause_and_preserves_wire_and_bytes() {
    #[derive(Clone, Default)]
    struct Log(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let fixture = interrupted("locked").await;
    let stage = fixture.config.state_root.join(STAGE);
    std::fs::create_dir(&stage).unwrap();
    std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o700)).unwrap();
    create(&stage.join("writer.lock"), b"");
    std::fs::set_permissions(
        stage.join("writer.lock"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let before = bytes(&fixture);
    let log = Log::default();
    let output = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || output.clone())
        .finish();
    let error =
        tracing::subscriber::with_default(subscriber, || select(&fixture.config).err().unwrap());
    assert_eq!(error.code, "mutation_ledger_format_hold");
    assert_eq!(
        error.detail,
        "runtime mutation ledger format requires verification"
    );
    assert!(!error.retryable);
    let wire = error.response(layer3_adapter::wire::WireOperation::RuntimeStatus);
    assert!(
        matches!(wire,layer3_adapter::wire::ResponseFrame::Failure(ref value)
        if value.code=="mutation_ledger_format_hold" && value.detail=="runtime mutation ledger format requires verification" && !value.retryable)
    );
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    for field in [
        "recovery.private_file",
        "inspect_existing",
        "observed_mode=416",
        "private_mode=false",
    ] {
        assert!(text.contains(field), "{text}");
    }
    assert!(!text.contains(fixture.root.to_str().unwrap()));
    assert_eq!(bytes(&fixture), before);
}
