use super::*;
use crate::store::{agent_log_integrity::TransactionLogError, suggestions};
use layer2_core::sandbox::suggestions::{context_entry_bytes, work_from_turn, MAX_CONTEXT_BYTES};

async fn fixture(
    content: &str,
    closed: i64,
) -> (Client, String, MongoSandboxStore, Agent, Vec<AgentLogEntry>) {
    let (client, name) = database("segment_suggestion").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.writer_policy = Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode":"accounts", "accounts":["owner"], "capabilities":["suggested_followup_admission"]
        }))
        .unwrap(),
    );
    let sandbox = sandbox("sbx_segments");
    let (_, mut agent) = worktree_and_agent(&sandbox.id);
    agent.conversation.conversation_id = Some("native-suggestions".into());
    agent.conversation.runtime_generation = Some(1);
    let kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-suggestions".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture goal".into(),
        },
        AgentLogKind::AgentMessage {
            content: content.into(),
        },
        AgentLogKind::AgentMessage {
            content: "fixture final answer".into(),
        },
        AgentLogKind::ModeChanged {
            mode: "fixture".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ];
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    store.create_sandbox(sandbox).await.unwrap();
    let (agent, entries) = install(&store, agent, entries, closed).await;
    (client, name, store, agent, entries)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn suggestions_keep_original_context_across_seals_and_skip_oversized_sources() {
    for (content, wanted) in [
        ("fixture reasoning".into(), true),
        ("x".repeat(MAX_CONTEXT_BYTES), false),
    ] {
        for closed in [0, 3] {
            let (client, name, store, agent, entries) = fixture(&content, closed).await;
            let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
            assert_eq!(
                work_from_turn(&agent, &parent, &entries[2..]).is_some(),
                wanted
            );
            let mut session = client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            suggestions::enqueue(&store, &agent, &entries[7..], &mut session)
                .await
                .unwrap();
            session.commit_transaction().await.unwrap();
            let row = store.suggestions.find_one(doc! {}).await.unwrap();
            assert_eq!(row.is_some(), wanted);
            if let Some(row) = row {
                let context = row
                    .get_document("record")
                    .unwrap()
                    .get_document("context")
                    .unwrap();
                assert_eq!(context.get_str("goal").unwrap(), "fixture goal");
                assert_eq!(context.get_str("answer").unwrap(), "fixture final answer");
            }
            assert_eq!(
                store.agent_log_range(&agent.id, 0, 8, 8).await.unwrap(),
                entries
            );
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn suggestions_never_treat_corrupt_or_missing_sealed_sources_as_optional_truncation() {
    for oversized in [false, true] {
        let content = if oversized {
            "x".repeat(MAX_CONTEXT_BYTES)
        } else {
            "fixture reasoning".into()
        };
        let (client, name, store, agent, entries) = fixture(&content, 3).await;
        // Sequence six follows the byte-budget boundary in the oversized
        // case. It must still be checked before optional work is skipped.
        let filter = doc! {"agent_id":agent.id.as_str(),"seq":6_i64};
        store
            .agent_log
            .update_one(
                filter.clone(),
                doc! {"$set":{"record.appended_at":"2026-09-06T00:00:00Z"}},
            )
            .await
            .unwrap();
        for missing in [false, true] {
            if missing {
                store.agent_log.delete_one(filter.clone()).await.unwrap();
            }
            let mut session = client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            assert!(matches!(
                suggestions::enqueue(&store, &agent, &entries[7..], &mut session).await,
                Err(TransactionLogError::Domain(
                    SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict
                ))
            ));
            session.abort_transaction().await.unwrap();
            assert_eq!(store.suggestions.count_documents(doc! {}).await.unwrap(), 0);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[test]
fn suggestion_source_budget_counts_serialized_bytes_including_escaping() {
    let mut entry = AgentLogEntry {
        agent_id: AgentId("agt_budget".into()),
        seq: 5,
        appended_at: NOW.into(),
        kind: AgentLogKind::AgentMessage {
            content: String::new(),
        },
    };
    let overhead = context_entry_bytes(&entry).unwrap();
    let AgentLogKind::AgentMessage { content } = &mut entry.kind else {
        unreachable!()
    };
    *content = "x".repeat(MAX_CONTEXT_BYTES - overhead);
    assert_eq!(context_entry_bytes(&entry), Some(MAX_CONTEXT_BYTES));
    let AgentLogKind::AgentMessage { content } = &mut entry.kind else {
        unreachable!()
    };
    content.push('x');
    assert_eq!(context_entry_bytes(&entry), None);
    let AgentLogKind::AgentMessage { content } = &mut entry.kind else {
        unreachable!()
    };
    *content = "\n".repeat(MAX_CONTEXT_BYTES / 2);
    assert_eq!(context_entry_bytes(&entry), None);
}
