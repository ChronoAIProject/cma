use super::*;
use crate::store::ledger_census::{observe, Member, Request};
use layer2_core::sandbox::ledger_census::LedgerFailureCause;

fn fixture() -> (Sandbox, Workspace, Worktree, Agent) {
    let parent = sandbox("sbx_census");
    let mut workspace = workspace(&parent, "wks_atomic", "project");
    workspace.status = WorkspaceStatus::Ready;
    workspace.work.step = WorkspaceWorkStep::None;
    let (worktree, mut agent) = worktree_and_agent(&parent.id);
    agent.status = AgentStatus::Failed;
    agent.failure = Some(TypedFailure {
        class: "adapter".into(),
        detail: "runtime mutation journal contains too many entries".into(),
    });
    agent.work.step = AgentWorkStep::RemoveWorktree;
    (parent, workspace, worktree, agent)
}

fn request(agent: &Agent) -> Request {
    Request {
        format: "runtime-ledger-census-request-v1".into(),
        agents: vec![Member {
            agent_id: agent.id.to_string(),
            sandbox_id: agent.sandbox_id.to_string(),
        }],
    }
}

#[test]
fn ledger_census_classifies_cause_and_parent_without_granting_recovery() {
    for (detail, cause) in [
        (
            "runtime mutation journal contains too many entries",
            LedgerFailureCause::JournalLifetimeBound,
        ),
        (
            "runtime mutation authority contains too many records",
            LedgerFailureCause::AuthorityLifetimeBound,
        ),
        (
            "runtime mutation ledger capacity is exhausted",
            LedgerFailureCause::SharedLedgerCapacity,
        ),
        ("unknown future refusal", LedgerFailureCause::OtherOrUnknown),
    ] {
        let (mut parent, workspace, worktree, mut agent) = fixture();
        agent.failure.as_mut().unwrap().detail = detail.into();
        parent.status = SandboxStatus::Retiring;
        let result = agent.ledger_census(&parent, &workspace, &worktree).unwrap();
        assert_eq!(result.cause, cause);
        assert_eq!(result.parent_state, SandboxStatus::Retiring);
        assert_eq!(
            result.recovery_route,
            "owner_disposition_then_supported_workload_migration"
        );
        assert!(result.cleanup_scheduled);
        assert!(!result.recovery_authorized);
        assert_eq!(result.filesystem_evidence, "not_observed");
        assert_eq!(result.provider_effects, "absence_not_proven");
        assert!(result.preserve.contains(&"complete_home_backup".into()));
        assert!(result
            .preserve
            .contains(&"pending_cleanup_intent_and_receipts_hold_before_restart".into()));
    }
}

#[test]
fn ledger_census_preserves_conversation_inputs_and_unknown_effects() {
    let (parent, workspace, worktree, mut agent) = fixture();
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    agent.work.attempts = 1;
    let result = agent.ledger_census(&parent, &workspace, &worktree).unwrap();
    assert!(result.conversation_present && result.attempted_or_recorded_work);
    assert!(result
        .preserve
        .contains(&"native_conversation_and_input_state_no_automatic_replay".into()));
    assert_eq!(
        result.recovery_route,
        "qualified_recovery_assessment_required"
    );
    assert!(!result.recovery_authorized);
    let mut foreign = workspace.clone();
    foreign.owner = SubjectId("foreign".into());
    assert!(agent.ledger_census(&parent, &foreign, &worktree).is_err());
    agent.status = AgentStatus::Deleted;
    let result = agent.ledger_census(&parent, &workspace, &worktree).unwrap();
    assert_eq!(
        result.recovery_route,
        "changed_agent_state_requires_reclassification"
    );
}

#[test]
fn ledger_census_retains_terminal_input_history_without_claiming_pending_work() {
    use layer2_core::sandbox::{FirstMessageFailure, FirstMessageRecord};
    for completed in [false, true] {
        let (parent, workspace, worktree, mut agent) = fixture();
        let mut input = FirstMessageRecord::new("fixture-key", "retained input", NOW).unwrap();
        input.fence_dispatch(1, 1, NOW.into()).unwrap();
        if completed {
            input.complete(MINUTE.into()).unwrap();
        } else {
            input
                .fail(
                    FirstMessageFailure::new("fixture", "retained failure", false, MINUTE).unwrap(),
                )
                .unwrap();
        }
        assert!(input.is_terminal());
        agent.conversation.first_message = Some(input);
        agent.conversation.last_seq = 1;
        let assessment = agent.ledger_census(&parent, &workspace, &worktree).unwrap();
        assert!(assessment.input_state_present);
        assert!(assessment
            .preserve
            .contains(&"native_conversation_and_input_state_no_automatic_replay".into()));
        assert!(!assessment.recovery_authorized);
        let encoded = serde_json::to_string(&assessment).unwrap();
        assert!(!encoded.contains("active_or_queued_input"));
        assert!(!encoded.contains("retained input"));
        assert!(!encoded.contains("retained failure"));
    }
}

fn documents() -> (Request, Vec<(&'static str, Document)>) {
    let (parent, workspace, worktree, agent) = fixture();
    (
        request(&agent),
        vec![
            (
                SANDBOXES,
                crate::store::sandboxes::sandbox_document(&parent, 0).unwrap(),
            ),
            (
                WORKSPACES,
                crate::store::workspaces::workspace_document(&workspace).unwrap(),
            ),
            (
                WORKTREES,
                crate::store::worktrees::worktree_document(&worktree).unwrap(),
            ),
            (
                AGENTS,
                crate::store::agents::agent_document(&agent).unwrap(),
            ),
        ],
    )
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn ledger_census_snapshot_is_read_only_and_keeps_missing_members() {
    let (client, name) = database("ledger_census").await;
    let db = client.database(&name);
    let (mut request, documents) = documents();
    for (collection, document) in &documents {
        db.collection::<Document>(collection)
            .insert_one(document)
            .await
            .unwrap();
    }
    request.agents.push(Member {
        agent_id: "missing-fixture".into(),
        sandbox_id: request.agents[0].sandbox_id.clone(),
    });
    let result = observe(&client, &db, &request, "a".repeat(64))
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 2);
    assert_eq!(
        result.rows[0].assessment.as_ref().unwrap().cause,
        LedgerFailureCause::JournalLifetimeBound
    );
    assert_eq!(result.rows[1].outcome, "agent_missing_preserve");
    assert!(result.rows[1].assessment.is_none());
    let again = observe(&client, &db, &request, "a".repeat(64))
        .await
        .unwrap();
    assert_eq!(result.source_sha256, again.source_sha256);
    for (collection, document) in &documents {
        let rows = db.collection::<Document>(collection);
        assert_eq!(rows.find_one(doc! {}).await.unwrap().unwrap(), *document);
        assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);
        assert_eq!(rows.list_index_names().await.unwrap(), ["_id_"]);
    }
    assert_eq!(db.list_collection_names().await.unwrap().len(), 4);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn ledger_census_unknown_schema_quarantine_and_changed_lineage_stay_preserved() {
    let (client, name) = database("census_unknown").await;
    for case in 0..5 {
        let db = client.database(&format!("{name}_{case}"));
        let (mut request, mut documents) = documents();
        match case {
            0 => {
                documents[3].1.insert("document_schema_version", 99);
            }
            1 => {
                documents[3].1.insert(
                    crate::store::agent_log_integrity::QUARANTINE_FIELD,
                    Bson::Null,
                );
            }
            2 => {
                request.agents[0].sandbox_id = "changed-parent".into();
            }
            3 => {
                documents.remove(0);
            }
            _ => {
                documents[3].1.insert("sandbox_id", "foreign");
            }
        }
        for (collection, document) in &documents {
            db.collection::<Document>(collection)
                .insert_one(document)
                .await
                .unwrap();
        }
        let result = observe(&client, &db, &request, "a".repeat(64))
            .await
            .unwrap();
        assert!(result.rows[0].assessment.is_none(), "case {case}");
        assert!(result.rows[0].outcome.ends_with("preserve"));
        for (collection, document) in &documents {
            assert_eq!(
                db.collection::<Document>(collection)
                    .find_one(doc! {})
                    .await
                    .unwrap()
                    .unwrap(),
                *document
            );
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn ledger_census_discovery_keeps_unknowns_and_has_no_writes_or_indexes() {
    use crate::store::ledger_census::discovery;
    let (client, name) = database("ledger_discovery").await;
    let db = client.database(&name);
    let (_, documents) = documents();
    let mut expected = Vec::new();
    for index in 0..5 {
        let mut row = documents[3].1.clone();
        row.insert("_id", format!("agt_{index:03}"));
        row.get_document_mut("record")
            .unwrap()
            .insert("id", format!("agt_{index:03}"));
        match index {
            1 => {
                row.insert("document_schema_version", 99);
            }
            2 => {
                row.insert(
                    crate::store::agent_log_integrity::QUARANTINE_FIELD,
                    Bson::Null,
                );
            }
            3 => {
                row.insert("sandbox_id", "foreign");
            }
            4 => {
                row.get_document_mut("record")
                    .unwrap()
                    .get_document_mut("failure")
                    .unwrap()
                    .insert("detail", "unrelated failure");
            }
            _ => {}
        }
        expected.push(row);
    }
    let agents = db.collection::<Document>(AGENTS);
    agents.insert_many(&expected).await.unwrap();
    let request = discovery::Request {
        format: "runtime-ledger-discovery-request-v1".into(),
        after_agent_id: None,
    };
    let page = discovery::observe(&client, &db, &request, "a".repeat(64))
        .await
        .unwrap();
    assert_eq!(page.candidates.agents.len(), 1);
    assert_eq!(page.candidates.agents[0].agent_id, "agt_000");
    assert_eq!(page.unknown.len(), 3);
    assert!(page.next.is_none());
    let encoded = serde_json::to_value(&page).unwrap();
    assert_eq!(encoded["other_failed"], 1);
    assert_eq!(encoded["scanned"], 5);
    assert_eq!(encoded["writes"], false);
    assert_eq!(encoded["recovery_authorized"], false);
    assert_eq!(agents.list_index_names().await.unwrap(), ["_id_"]);
    assert_eq!(agents.count_documents(doc! {}).await.unwrap(), 5);
    assert_eq!(db.list_collection_names().await.unwrap(), [AGENTS]);
    for original in expected {
        assert_eq!(
            agents
                .find_one(doc! {"_id": original.get_str("_id").unwrap()})
                .await
                .unwrap()
                .unwrap(),
            original
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn ledger_census_discovery_continues_across_mongo_byte_limited_batches() {
    use crate::store::ledger_census::discovery;
    let (client, name) = database("ledger_discovery_bytes").await;
    let db = client.database(&name);
    let (_, documents) = documents();
    let agents = db.collection::<Document>(AGENTS);
    let detail = format!(
        "runtime mutation journal contains too many entries {}",
        "x".repeat(280 * 1024)
    );
    for index in 0..65 {
        let mut row = documents[3].1.clone();
        row.insert("_id", format!("agt_{index:03}"));
        row.get_document_mut("record")
            .unwrap()
            .insert("id", format!("agt_{index:03}"));
        row.get_document_mut("record")
            .unwrap()
            .get_document_mut("failure")
            .unwrap()
            .insert("detail", &detail);
        agents.insert_one(row).await.unwrap();
    }
    // Non-string IDs are outside this discovery's declared scope, so it must
    // never be used to certify the historical exact-ID cohort's completeness.
    agents
        .insert_one(doc! {"_id": 17, "status":"failed"})
        .await
        .unwrap();
    let mut request = discovery::Request {
        format: "runtime-ledger-discovery-request-v1".into(),
        after_agent_id: None,
    };
    let first = discovery::observe(&client, &db, &request, "a".repeat(64))
        .await
        .unwrap();
    assert_eq!(first.candidates.agents.len(), 64);
    assert_eq!(first.next.as_deref(), Some("agt_063"));
    request.after_agent_id = first.next;
    let second = discovery::observe(&client, &db, &request, "b".repeat(64))
        .await
        .unwrap();
    assert_eq!(second.candidates.agents.len(), 1);
    assert_eq!(second.candidates.agents[0].agent_id, "agt_064");
    assert!(second.next.is_none());
    assert_eq!(agents.count_documents(doc! {}).await.unwrap(), 66);
    assert_eq!(agents.list_index_names().await.unwrap(), ["_id_"]);
    // The source-byte budget fails the entire page; no partial candidate list
    // can look complete when the bounded observation could not finish.
    agents
        .update_many(
            doc! {"_id":{"$type":"string"}},
            doc! {"$set":{"record.failure.detail": "x".repeat(600 * 1024)}},
        )
        .await
        .unwrap();
    request.after_agent_id = None;
    assert!(discovery::observe(&client, &db, &request, "c".repeat(64))
        .await
        .is_err());
    db.drop().await.unwrap();
}
