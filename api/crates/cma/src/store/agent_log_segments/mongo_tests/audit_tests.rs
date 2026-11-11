use super::*;
use segments::audit::{self, test_support as hooks};

pub(super) async fn conversation(
    store: &MongoSandboxStore,
    messages: usize,
    size: usize,
) -> (Agent, Vec<AgentLogEntry>) {
    let mut agent = agent();
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.active_turn = Some(3);
    let mut kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture".into(),
        },
    ];
    kinds.extend((0..messages).map(|_| AgentLogKind::AgentMessage {
        content: "x".repeat(size),
    }));
    let rows = kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    fixtures::install(store, agent, rows, 0, 1).await
}
async fn expire(db: &mongodb::Database, agent: &Agent, audit: &str) {
    db.collection::<Document>(AUDITS)
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"audit_id":audit},
            doc! {"$set":{"expires_at_ms":0_i64}},
        )
        .await
        .unwrap();
}
async fn progress(db: &mongodb::Database, agent: &Agent, audit: &str) -> Document {
    db.collection::<Document>(AUDITS)
        .find_one(doc! {"agent_id":agent.id.as_str(),"audit_id":audit})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn audit_resumes_units_and_installs_only_an_exact_complete_capture() {
    let (client, name) = database("audit_units").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    let original = store
        .agent_log
        .find(doc! {})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    for expected in ["running", "running", "running", "complete", "complete"] {
        assert_eq!(
            audit::run_unit(&client, &db, agent.id.as_str(), "units", false)
                .await
                .unwrap(),
            expected
        );
        if expected == "running" {
            assert!(audit::install(&client, &db, agent.id.as_str(), "units")
                .await
                .is_err());
        }
    }
    let record = progress(&db, &agent, "units").await;
    assert_eq!(
        record
            .get_document("completed")
            .unwrap()
            .get_i64("checked_entries")
            .unwrap(),
        8
    );
    assert_eq!(
        record
            .get_document("completed")
            .unwrap()
            .get_i64("next_ordinal")
            .unwrap(),
        4
    );
    assert_eq!(
        audit::install(&client, &db, agent.id.as_str(), "units")
            .await
            .unwrap(),
        "installed"
    );
    assert_eq!(
        audit::install(&client, &db, agent.id.as_str(), "units")
            .await
            .unwrap(),
        "installed"
    );
    assert_eq!(
        store.agent_log_range(&agent.id, 0, 8, 8).await.unwrap(),
        entries
    );
    assert_eq!(
        store
            .agent_log
            .find(doc! {})
            .sort(doc! {"seq":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        original
    );
    let head = collection(&store, HEADS)
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        head.get_str("evidence_generation").unwrap(),
        record.get_str("result_generation").unwrap()
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn audit_partial_generation_restart_preserves_completed_prefix_and_discards_partial_authority(
) {
    let (client, name) = database("audit_partial").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 2).await;
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "partial", false)
            .await
            .unwrap(),
        "running"
    );
    let claim = hooks::claim(&client, &db, agent.id.as_str(), "partial")
        .await
        .unwrap();
    let partial = hooks::partial(
        &client,
        &db,
        claim,
        layer2_core::sandbox::AgentLogEvidenceChange::Queued {
            sequence: 2,
            consumed_by: None,
        },
    )
    .await
    .unwrap();
    let abandoned = partial
        .get_document("stage")
        .unwrap()
        .get_str("generation")
        .unwrap()
        .to_string();
    assert_eq!(
        partial
            .get_document("completed")
            .unwrap()
            .get_i64("checked_entries")
            .unwrap(),
        2
    );
    assert!(audit::install(&client, &db, agent.id.as_str(), "partial")
        .await
        .is_err());
    expire(&db, &agent, "partial").await;
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "partial", false)
            .await
            .unwrap(),
        "running"
    );
    let advanced = progress(&db, &agent, "partial").await;
    assert_eq!(
        advanced
            .get_document("completed")
            .unwrap()
            .get_i64("checked_entries")
            .unwrap(),
        4
    );
    assert!(hooks::touch(&client, &db, partial).await.is_err());
    assert_eq!(progress(&db, &agent, "partial").await, advanced);
    assert_eq!(
        db.collection::<Document>(EVIDENCE)
            .count_documents(doc! {"evidence_generation":abandoned})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        db.collection::<Document>(EVIDENCE)
            .count_documents(
                doc! {"evidence_generation":advanced.get_str("result_generation").unwrap()}
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "partial", false)
            .await
            .unwrap(),
        "complete"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn validated_promotion_resumes_without_source_scan_and_rejects_stale_batch() {
    let (client, name) = database("audit_promotion").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = conversation(&store, 180, 1).await;
    let original = collection(&store, HEADS)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let claim = hooks::claim(&client, &db, agent.id.as_str(), "promotion")
        .await
        .unwrap();
    let validated = hooks::collect(&client, &db, claim).await.unwrap();
    assert_eq!(
        validated
            .get_document("stage")
            .unwrap()
            .get_str("phase")
            .unwrap(),
        "promoting"
    );
    assert!(audit::install(&client, &db, agent.id.as_str(), "promotion")
        .await
        .is_err());
    let (advanced, done) = hooks::promote(&client, &db, validated.clone())
        .await
        .unwrap();
    assert!(!done);
    assert!(hooks::promote(&client, &db, validated).await.is_err());
    assert_eq!(progress(&db, &agent, "promotion").await, advanced);
    assert_eq!(
        collection(&store, HEADS)
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap(),
        original
    );
    expire(&db, &agent, "promotion").await;
    crate::store::mongo_read_telemetry::isolated_test(async {
        assert_eq!(
            audit::run_unit(&client, &db, agent.id.as_str(), "promotion", false)
                .await
                .unwrap(),
            "complete"
        );
        assert_eq!(
            crate::store::mongo_read_telemetry::snapshot("agent_log"),
            [0; 4]
        );
    })
    .await;
    assert_eq!(
        db.collection::<Document>(EVIDENCE)
            .count_documents(
                doc! {"evidence_generation":advanced.get_str("result_generation").unwrap()}
            )
            .await
            .unwrap(),
        182
    );
    assert_eq!(
        audit::install(&client, &db, agent.id.as_str(), "promotion")
            .await
            .unwrap(),
        "installed"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn cancelled_or_failed_batch_cannot_advance_progress_or_write_evidence() {
    let (client, name) = database("audit_atomic").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 1).await;
    let claim = hooks::claim(&client, &db, agent.id.as_str(), "atomic")
        .await
        .unwrap();
    assert!(hooks::write_invalid(&client, &db, claim.clone())
        .await
        .is_err());
    assert_eq!(progress(&db, &agent, "atomic").await, claim);
    assert_eq!(
        db.collection::<Document>(EVIDENCE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "atomic", true)
            .await
            .unwrap(),
        "cancelled"
    );
    let cancelled = progress(&db, &agent, "atomic").await;
    assert!(hooks::touch(&client, &db, claim).await.is_err());
    assert_eq!(progress(&db, &agent, "atomic").await, cancelled);
    assert_eq!(
        audit::run_unit(&client, &db, agent.id.as_str(), "atomic", false)
            .await
            .unwrap(),
        "cancelled"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn audit_quarantines_untouched_corruption_and_install_refuses_newer_aggregate() {
    for damaged in [false, true] {
        let (client, name) = database("audit_refusal").await;
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, _) = segmented(&store, 1).await;
        if damaged {
            store
                .agent_log
                .update_one(
                    doc! {"seq":2_i64},
                    doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
                )
                .await
                .unwrap();
            assert!(store.agent_log_range(&agent.id, 2, 3, 1).await.is_ok());
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "refusal", false)
                    .await
                    .unwrap(),
                "corrupt"
            );
            assert!(store.agent_log_range(&agent.id, 2, 3, 1).await.is_err());
        } else {
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "refusal", false)
                    .await
                    .unwrap(),
                "running"
            );
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "refusal", false)
                    .await
                    .unwrap(),
                "complete"
            );
            store
                .agents
                .update_one(
                    doc! {"_id":agent.id.as_str()},
                    doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
                )
                .await
                .unwrap();
            assert!(audit::install(&client, &db, agent.id.as_str(), "refusal")
                .await
                .is_err());
            assert_eq!(
                collection(&store, HEADS)
                    .find_one(doc! {})
                    .await
                    .unwrap()
                    .unwrap()
                    .get_str("evidence_generation")
                    .unwrap(),
                "initial"
            );
        }
        db.drop().await.unwrap();
    }
}
