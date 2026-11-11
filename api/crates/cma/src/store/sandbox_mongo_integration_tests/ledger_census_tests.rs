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
