use super::*;

async fn interrupted(step: &str) -> Fixture {
    let fixture = Fixture::new();
    set_failure(&fixture.config, step);
    assert!(fixture.open().await.is_err(), "{step}");
    set_failure(&fixture.config, "");
    fixture
}

fn database(fixture: &Fixture) -> PathBuf {
    let stage = fixture.config.state_root.join(STAGE);
    if stage.exists() {
        stage
    } else {
        fixture.config.state_root.join(DATABASE)
    }
    .join("ledger.sqlite3")
}

#[tokio::test]
async fn fresh_intent_completes_each_publication_boundary_and_reopens_the_same_store() {
    for step in [
        "intent",
        "barrier-0",
        "barrier-1",
        "barrier-2",
        "barrier-3",
        "barrier-4",
        "barriers",
        "renamed",
        "temporary",
        "committed",
        "intent-removal",
        "intent-consumed",
    ] {
        let fixture = interrupted(step).await;
        let intent = fixture.config.state_root.join(publication::INTENT);
        let descriptor_path = if intent.exists() {
            intent.clone()
        } else {
            fixture.config.state_root.join(MANIFEST)
        };
        let bytes = std::fs::read(descriptor_path).unwrap();
        let descriptor = Descriptor::decode(&fixture.config, &bytes).unwrap();
        let ledger = fixture.open().await.unwrap().unwrap();
        assert_eq!(ledger.store_id().await.unwrap(), descriptor.store, "{step}");
        assert!(!intent.exists(), "{step}");
        descriptor.verify(descriptor.base_markers()).unwrap();
        close(ledger).await;
        let ledger = fixture.open().await.unwrap().unwrap();
        assert_eq!(ledger.store_id().await.unwrap(), descriptor.store, "{step}");
        close(ledger).await;
    }
}

#[tokio::test]
async fn second_interruption_during_proven_publication_preserves_recovery() {
    for step in [
        "barrier-0",
        "recovery-renamed",
        "recovery-committed",
        "intent-removal",
        "intent-consumed",
    ] {
        let fixture = interrupted("intent").await;
        let bytes = std::fs::read(fixture.config.state_root.join(publication::INTENT)).unwrap();
        let descriptor = Descriptor::decode(&fixture.config, &bytes).unwrap();
        set_failure(&fixture.config, step);
        assert!(fixture.open().await.is_err(), "{step}");
        set_failure(&fixture.config, "");
        let ledger = fixture.open().await.unwrap().unwrap();
        assert_eq!(ledger.store_id().await.unwrap(), descriptor.store, "{step}");
        close(ledger).await;
    }
}

#[tokio::test]
async fn fresh_proof_rejects_schema_identity_and_every_effect_table_without_rewriting_database() {
    let changes = [
        "DELETE FROM metadata",
        "DELETE FROM accounting",
        "UPDATE metadata SET store_id=zeroblob(16)",
        "UPDATE metadata SET generation=x'0000000000000002'",
        "UPDATE metadata SET state_root='/other-retained-state'",
        "UPDATE metadata SET admission='import_held'",
        "UPDATE accounting SET revision=1",
        "UPDATE accounting SET reserved=1",
        "UPDATE accounting SET operations=1",
        "UPDATE accounting SET authority=1",
        "UPDATE accounting SET terminal=1",
        "INSERT INTO authority VALUES (x'0000000000000001','resource',x'0000000000000001',x'01',NULL,NULL)",
        "INSERT INTO scope_budget VALUES (x'0000000000000001',0,0)",
        "INSERT INTO cleanup_budget VALUES (x'0000000000000001','resource',1)",
        "INSERT INTO operations VALUES (x'0000000000000001','operation',x'01',0,1,NULL)",
        "INSERT INTO logical_processes VALUES (x'0000000000000001','agent','/cwd','/home')",
        "DROP TABLE cleanup_budget",
        "DROP INDEX outstanding",
        "CREATE TRIGGER unexpected AFTER UPDATE ON accounting BEGIN SELECT 1; END",
    ];
    for step in ["intent", "committed"] {
        for change in changes {
            let fixture = interrupted(step).await;
            let path = database(&fixture);
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch(change).unwrap();
            db.close().unwrap();
            let before = std::fs::read(&path).unwrap();
            let intent = fixture.config.state_root.join(publication::INTENT);
            let intent_before = std::fs::read(&intent).unwrap();
            let error = fixture.open().await.err().unwrap();
            assert_eq!(
                error.code, "mutation_ledger_format_hold",
                "{step}: {change}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before, "{step}: {change}");
            assert_eq!(std::fs::read(&intent).unwrap(), intent_before);
        }
    }
}

#[tokio::test]
async fn intent_cannot_normalize_an_older_database_generation() {
    let mut fixture = Fixture::new();
    fixture.config.generation = 2;
    set_failure(&fixture.config, "intent");
    assert!(fixture.open().await.is_err());
    set_failure(&fixture.config, "");
    let path = database(&fixture);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE metadata SET generation=x'0000000000000001'", [])
        .unwrap();
    db.close().unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(fixture.open().await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn malformed_foreign_or_unsafe_intent_and_unknown_artifacts_remain_held() {
    use std::os::unix::fs::PermissionsExt;
    for kind in [
        "partial-intent",
        "foreign-store",
        "foreign-home",
        "future-generation",
        "mode",
        "hardlink",
        "unknown",
        "journal",
        "retirement",
        "barrier",
        "partial-generation",
    ] {
        let fixture = interrupted("barrier-0").await;
        let intent = fixture.config.state_root.join(publication::INTENT);
        match kind {
            "partial-intent" => std::fs::write(&intent, b"{\"version\":").unwrap(),
            "foreign-store" | "foreign-home" | "future-generation" => {
                let mut descriptor =
                    Descriptor::decode(&fixture.config, &std::fs::read(&intent).unwrap()).unwrap();
                match kind {
                    "foreign-store" => descriptor.store = uuid::Uuid::new_v4().to_string(),
                    "foreign-home" => descriptor.home = "b".repeat(43),
                    _ => descriptor.born = 2,
                }
                std::fs::write(&intent, descriptor.bytes().unwrap()).unwrap();
            }
            "mode" => {
                std::fs::set_permissions(&intent, std::fs::Permissions::from_mode(0o640)).unwrap()
            }
            "hardlink" => std::fs::hard_link(&intent, fixture.root.join("another-link")).unwrap(),
            "unknown" => {
                std::fs::write(fixture.config.state_root.join("unexplained"), b"retained").unwrap()
            }
            "journal" => std::fs::write(
                database(&fixture).with_file_name("ledger.sqlite3-journal"),
                b"retained",
            )
            .unwrap(),
            "retirement" => std::fs::write(
                fixture
                    .config
                    .state_root
                    .join("mutation-retirement-v1.current.json"),
                b"retained",
            )
            .unwrap(),
            "barrier" => std::fs::write(
                fixture
                    .config
                    .checkpoint_root
                    .join("runtime-state")
                    .join(BARRIER),
                b"retained",
            )
            .unwrap(),
            _ => {
                std::fs::create_dir(fixture.config.state_root.join("process-generation-2")).unwrap()
            }
        }
        let before = std::fs::read(&intent).unwrap();
        let db_before = std::fs::read(database(&fixture)).unwrap();
        assert!(fixture.open().await.is_err(), "{kind}");
        assert_eq!(std::fs::read(&intent).unwrap(), before, "{kind}");
        assert_eq!(
            std::fs::read(database(&fixture)).unwrap(),
            db_before,
            "{kind}"
        );
        assert!(!fixture.config.state_root.join(MANIFEST).exists());
    }
}

#[tokio::test]
async fn actual_supervisor_consumes_intent_before_accepting_installation() {
    let fixture = interrupted("temporary").await;
    let supervisor = crate::supervisor::Supervisor::start(
        fixture.config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .await
    .unwrap();
    assert!(!fixture.config.state_root.join(publication::INTENT).exists());
    let response = supervisor
        .dispatch(layer3_adapter::wire::RequestFrame::Install(
            layer3_adapter::InstallRequest::new("recovered-fresh-install", vec![]),
        ))
        .await;
    assert!(
        matches!(response, layer3_adapter::wire::ResponseFrame::Install(_)),
        "{response:?}"
    );
}

pub(super) fn replace_path(config: &RuntimeConfig, phase: &str) {
    let selected = FAIL.lock().unwrap().get(&config.state_root).cloned();
    if selected.as_deref() != Some(&format!("replace-{phase}")) {
        return;
    }
    let source = if config.state_root.join(STAGE).exists() {
        STAGE
    } else {
        DATABASE
    };
    let path = config.state_root.join(source);
    std::fs::rename(&path, config.state_root.join("retained-original-database")).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("foreign-bytes"), b"do-not-change").unwrap();
}

#[tokio::test]
async fn replaced_verified_database_never_publishes_or_consumes_its_proof() {
    for step in ["intent", "renamed"] {
        let fixture = interrupted(step).await;
        let original = std::fs::read(database(&fixture)).unwrap();
        let intent = fixture.config.state_root.join(publication::INTENT);
        let intent_before = std::fs::read(&intent).unwrap();
        set_failure(&fixture.config, "replace-verified");
        assert!(fixture.open().await.is_err());
        set_failure(&fixture.config, "");
        let foreign = if step == "intent" { STAGE } else { DATABASE };
        assert_eq!(
            std::fs::read(
                fixture
                    .config
                    .state_root
                    .join(foreign)
                    .join("foreign-bytes")
            )
            .unwrap(),
            b"do-not-change"
        );
        assert_eq!(
            std::fs::read(
                fixture
                    .config
                    .state_root
                    .join("retained-original-database/ledger.sqlite3")
            )
            .unwrap(),
            original
        );
        assert_eq!(std::fs::read(&intent).unwrap(), intent_before);
        assert!(!fixture.config.state_root.join(MANIFEST).exists());
    }
    for phase in ["renamed", "committed"] {
        let fixture = interrupted("intent").await;
        set_failure(&fixture.config, &format!("replace-{phase}"));
        assert!(fixture.open().await.is_err());
        set_failure(&fixture.config, "");
        assert!(fixture.config.state_root.join(publication::INTENT).exists());
        assert_eq!(
            std::fs::read(
                fixture
                    .config
                    .state_root
                    .join(DATABASE)
                    .join("foreign-bytes")
            )
            .unwrap(),
            b"do-not-change"
        );
        assert_eq!(
            fixture.config.state_root.join(MANIFEST).exists(),
            phase == "committed"
        );
    }
}
