use super::semantic::{add, authority, fixture, journal, remove};
use super::*;
use crate::mutation_journal::{Record, Status, Subject};
use crate::mutation_ledger::Ledger;
use layer3_adapter::{AgentCloseRequest, AgentSlot};
use rusqlite::{Connection, OpenFlags};

fn plan(fixture: &Fixture) -> (PathBuf, String) {
    let path = fixture.root.join("plan");
    let report = diagnostics::assert_ok(|| plan::create(&fixture.config, &path));
    (path, report["plan_sha256"].as_str().unwrap().into())
}
fn database(output: &Path) -> Connection {
    Connection::open_with_flags(
        output.join("database/ledger.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

#[tokio::test]
async fn numbered_open_with_missing_or_moved_predecessor_keeps_import_held() {
    use crate::mutation_journal::Operation;
    use layer3_adapter::wire::{AgentCloseReceipt, AgentOpenReceipt};
    use layer3_adapter::AgentProcessAction::{Close, Open};

    for previous_workspace in [None, Some("different-workspace")] {
        let fixture = fixture();
        let config = &fixture.config;
        let agent = AgentSlot::new("agent");
        let opened = |incarnation, workspace| {
            let cwd = crate::worktree::expected_path(&config.home, workspace, agent.as_str())
                .to_string_lossy()
                .into_owned();
            let home = config
                .agent_home(agent.as_str())
                .to_string_lossy()
                .into_owned();
            let operation = Operation {
                operation_id: Open.operation_id(agent.as_str(), incarnation),
                request_sha256: "a".repeat(64),
                payload_sha256: "b".repeat(64),
                subject: Subject::OpenAgent {
                    agent_id: agent.as_str().into(),
                    cwd: cwd.clone(),
                    home: home.clone(),
                },
            };
            Record::started(config, &operation)
                .unwrap()
                .completed(&AgentOpenReceipt {
                    operation_id: operation.operation_id,
                    request_sha256: operation.request_sha256,
                    agent_id: agent.clone(),
                    cwd,
                    home,
                    conversation_id: None,
                })
                .unwrap()
        };
        journal(config, &opened(2, "workspace"));
        if let Some(workspace) = previous_workspace {
            journal(config, &opened(1, workspace));
            let request = AgentCloseRequest::new(Close.operation_id(agent.as_str(), 1), agent);
            let record = Record::started(config, &Operation::close_agent(&request))
                .unwrap()
                .completed(&AgentCloseReceipt {
                    operation_id: request.operation_id,
                    request_sha256: request.request_sha256,
                    agent_id: request.agent_id,
                })
                .unwrap();
            journal(config, &record);
        }
        assert_eq!(
            crate::mutation_journal::active_agents(config)
                .unwrap_err()
                .code,
            "agent_process_history_invalid"
        );
        let (plan, digest) = plan(&fixture);
        let output = fixture.root.join("staged");
        let error = stage::create(config.clone(), &plan, &digest, &output)
            .await
            .unwrap_err();
        assert_eq!(error.code, "agent_process_history_invalid");
        assert!(!output.join("ready.json").exists());
    }
}

#[tokio::test]
async fn imported_terminal_bytes_and_unsigned_authority_survive_with_a_durable_hold() {
    let fixture = fixture();
    for fence in 1..=4097 {
        authority(&fixture.config, fence, &add());
    }
    authority(&fixture.config, 5000, &remove());
    authority(&fixture.config, u64::MAX, &remove());
    let started = Record::started(&fixture.config, &add()).unwrap();
    let canonical = journal(&fixture.config, &started);
    let failed = started
        .failed(
            &RuntimeError::rejected("effect_failed", "effect failed"),
            false,
        )
        .unwrap();
    let bytes = serde_json::to_vec_pretty(&failed).unwrap();
    std::fs::write(
        canonical.parent().unwrap().join(".mutation-journal-1.tmp"),
        &bytes,
    )
    .unwrap();
    let before = fixture.scan().unwrap().digest;
    let (plan, digest) = plan(&fixture);
    let output = fixture.root.join("staged");
    let summary = stage::create(fixture.config.clone(), &plan, &digest, &output)
        .await
        .unwrap();
    assert_eq!(summary["admission"], "import_held");
    assert_eq!(summary["operations"], 2); // Includes the authority-only removal.
    assert_eq!(summary["terminal"], 1);
    assert_eq!(summary["authority_resources"], 1);
    assert_eq!(before, fixture.scan().unwrap().digest);
    let db = database(&output);
    let actual: Vec<u8> = db
        .query_row("SELECT record FROM operations WHERE status=3", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(actual, bytes);
    let high: Vec<u8> = db
        .query_row("SELECT high_water FROM authority", [], |r| r.get(0))
        .unwrap();
    assert_eq!(high, u64::MAX.to_be_bytes());
    let pending: i64 = db
        .query_row(
            "SELECT count(*) FROM operations WHERE status=0 AND record IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1);
    let mut replacement = fixture.config.clone();
    replacement.generation += 1;
    assert_eq!(
        Ledger::open(output.join("database"), replacement, false, stage::PAGES)
            .await
            .err()
            .unwrap()
            .code,
        "mutation_ledger_import_held"
    );
    let generation: Vec<u8> = db
        .query_row("SELECT generation FROM metadata", [], |r| r.get(0))
        .unwrap();
    assert_eq!(generation, fixture.config.generation.to_be_bytes());
    assert_eq!(
        Record::decode(&std::fs::read(canonical).unwrap(), &fixture.config)
            .unwrap()
            .status(),
        Status::Started
    );
}

#[tokio::test]
async fn interrupted_staging_is_always_held_without_a_ready_artifact() {
    let fixture = fixture();
    authority(&fixture.config, 1, &add());
    journal(
        &fixture.config,
        &Record::started(&fixture.config, &add()).unwrap(),
    );
    let before = fixture.scan().unwrap().digest;
    let (plan, digest) = plan(&fixture);
    for fail_at in 0..6 {
        let output = fixture.root.join(format!("interrupted-{fail_at}"));
        let mut step = 0;
        let result = stage::create_checked(
            fixture.config.clone(),
            &plan,
            &digest,
            &output,
            stage::PAGES,
            |_| {
                let fail = step == fail_at;
                step += 1;
                if fail {
                    Err(invalid())
                } else {
                    Ok(())
                }
            },
        )
        .await;
        assert!(result.is_err(), "boundary {fail_at}");
        assert!(!output.join("ready.json").exists());
        assert_eq!(before, fixture.scan().unwrap().digest);
        let admission: String = database(&output)
            .query_row("SELECT admission FROM metadata", [], |r| r.get(0))
            .unwrap();
        assert_eq!(admission, "import_held");
        assert!(
            stage::create(fixture.config.clone(), &plan, &digest, &output)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn reservations_must_fit_before_a_staged_import_becomes_ready() {
    let fixture = fixture();
    authority(&fixture.config, 1, &add());
    let (plan, digest) = plan(&fixture);
    let output = fixture.root.join("full");
    let result = stage::create_checked(
        fixture.config.clone(),
        &plan,
        &digest,
        &output,
        1024,
        |_| Ok(()),
    )
    .await;
    assert_eq!(result.unwrap_err().code, "mutation_ledger_capacity");
    assert!(!output.join("ready.json").exists());
    let db = database(&output);
    let (held, reserved): (String, i64) = db
        .query_row(
            "SELECT admission,reserved FROM metadata,accounting",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(held, "import_held");
    assert!(reserved > 4 * 1024 * 1024); // Outcome and future cleanup were retained.
    assert!(!fixture
        .config
        .state_root
        .join("mutation-ledger-v2.current.json")
        .exists());
}

#[tokio::test]
async fn journal_only_records_and_historical_processes_keep_v1_close_semantics() {
    let fixture = fixture();
    let close = crate::mutation_journal::Operation::close_agent(&AgentCloseRequest::new(
        "agent:agent:close:v1",
        AgentSlot::new("agent"),
    ));
    for generation in [2, 7] {
        let mut config = fixture.config.clone();
        config.generation = generation;
        let open = crate::mutation_journal::Operation {
            operation_id: "agent:agent:open:v1".into(),
            request_sha256: "a".repeat(64),
            payload_sha256: "b".repeat(64),
            subject: Subject::OpenAgent {
                agent_id: "agent".into(),
                cwd: crate::worktree::expected_path(&config.home, "workspace", "agent")
                    .to_string_lossy()
                    .into(),
                home: config.agent_home("agent").to_string_lossy().into(),
            },
        };
        journal(
            &config,
            &Record::started(&config, &open)
                .unwrap()
                .failed(
                    &RuntimeError::rejected("agent_open_outcome_unknown", "unknown"),
                    true,
                )
                .unwrap(),
        );
        authority(&config, u64::MAX, &close);
        if generation == 2 {
            journal(
                &config,
                &Record::started(&config, &close)
                    .unwrap()
                    .completed(&layer3_adapter::wire::AgentCloseReceipt {
                        operation_id: close.operation_id.clone(),
                        request_sha256: close.request_sha256.clone(),
                        agent_id: AgentSlot::new("agent"),
                    })
                    .unwrap(),
            );
        }
    }
    let (plan, digest) = plan(&fixture);
    let output = fixture.root.join("staged");
    let summary = stage::create(fixture.config.clone(), &plan, &digest, &output)
        .await
        .unwrap();
    assert_eq!(summary["operations"], 4);
    assert_eq!(summary["terminal"], 3);
    assert_eq!(summary["logical_processes"], 1);
    let db = database(&output);
    let scope: Vec<u8> = db
        .query_row("SELECT scope FROM logical_processes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(scope, 7u64.to_be_bytes());
}

#[tokio::test]
async fn terminal_history_past_the_v1_limit_does_not_consume_outstanding_slots() {
    let fixture = fixture();
    for index in 0..1025 {
        let agent = format!("agent-{index}");
        let close = crate::mutation_journal::Operation::close_agent(&AgentCloseRequest::new(
            format!("agent:{agent}:close:v1"),
            AgentSlot::new(agent),
        ));
        journal(
            &fixture.config,
            &Record::started(&fixture.config, &close)
                .unwrap()
                .failed(
                    &RuntimeError::rejected("effect_failed", "effect failed"),
                    false,
                )
                .unwrap(),
        );
    }
    let (plan, digest) = plan(&fixture);
    let output = fixture.root.join("staged");
    let summary = stage::create(fixture.config.clone(), &plan, &digest, &output)
        .await
        .unwrap();
    assert_eq!(summary["operations"], 1025);
    assert_eq!(summary["terminal"], 1025);
    assert_eq!(summary["reserved_bytes"], 0);
    let (pending, scope_rows): (i64, i64) = database(&output)
        .query_row(
            "SELECT coalesce(sum(pending),0),count(*) FROM scope_budget",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(pending, 0);
    assert_eq!(scope_rows, 0);
}

#[tokio::test]
async fn changed_backup_after_database_close_cannot_publish_ready() {
    let fixture = fixture();
    authority(&fixture.config, 1, &add());
    let (plan, digest) = plan(&fixture);
    let output = fixture.root.join("staged");
    let result = stage::create_checked(
        fixture.config.clone(),
        &plan,
        &digest,
        &output,
        stage::PAGES,
        |boundary| {
            if matches!(boundary, stage::Boundary::Ready) {
                std::fs::remove_file(plan.join("00000000.record")).unwrap();
            }
            Ok(())
        },
    )
    .await;
    assert!(result.is_err());
    assert!(!output.join("ready.json").exists());
    let admission: String = database(&output)
        .query_row("SELECT admission FROM metadata", [], |r| r.get(0))
        .unwrap();
    assert_eq!(admission, "import_held");
}

#[tokio::test]
async fn changed_database_between_verification_and_ready_publication_is_refused() {
    for replace in [false, true] {
        let fixture = fixture();
        authority(&fixture.config, 1, &add());
        let (plan, digest) = plan(&fixture);
        let output = fixture.root.join("staged");
        let result = stage::create_checked(
            fixture.config.clone(),
            &plan,
            &digest,
            &output,
            stage::PAGES,
            |boundary| {
                let change = if replace {
                    matches!(boundary, stage::Boundary::Closed)
                } else {
                    matches!(boundary, stage::Boundary::Ready)
                };
                if change {
                    let database = output.join("database/ledger.sqlite3");
                    if replace {
                        std::fs::rename(&database, output.join("original-database")).unwrap();
                        std::fs::write(&database, b"replacement").unwrap();
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))
                            .unwrap();
                    } else {
                        use std::io::Write;
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&database)
                            .unwrap()
                            .write_all(b"drift")
                            .unwrap();
                    }
                }
                Ok(())
            },
        )
        .await;
        assert_eq!(
            result.unwrap_err().code,
            "mutation_ledger_migration_changed"
        );
        assert!(!output.join("ready.json").exists());
    }
}
