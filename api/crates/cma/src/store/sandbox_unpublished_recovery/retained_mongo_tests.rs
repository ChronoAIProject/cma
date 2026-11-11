use super::*;
use crate::store::agent_log::log_document;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_recovery_mongo_snapshot_preserves_published_history_without_admission() {
    let (client, name, store) = database().await;
    let request = fixture(&store, "sbx_retained_read").await;
    let agent = &request.snapshot.agents[0];
    let entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 2,
        },
    };
    store
        .agent_log
        .replace_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 },
            log_document(&entry).unwrap(),
        )
        .await
        .unwrap();
    let before = store
        .sandboxes
        .find_one(doc! { "_id": request.snapshot.sandbox.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let held = store
        .retained_runtime_recovery_snapshot(&request.snapshot.sandbox.id)
        .await
        .unwrap();
    assert_eq!(held.agents, request.snapshot.agents);
    assert_eq!(held.sandbox, request.snapshot.sandbox);
    assert!(store
        .unpublished_runtime_recovery_snapshot(&request.snapshot.sandbox.id)
        .await
        .is_err());
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! { "_id": request.snapshot.sandbox.id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_recovery_mongo_snapshot_validates_terminal_child_logs_and_parent_events() {
    let (client, name, store) = database().await;
    let request = fixture(&store, "sbx_retained_integrity").await;
    let id = &request.snapshot.sandbox.id;
    let mut terminal = ready_agent(&request.snapshot.sandbox, "agt_terminal_evidence");
    terminal.status = AgentStatus::Stopped;
    insert_agent(&store, &terminal, NOW).await;
    assert_eq!(
        store
            .retained_runtime_recovery_snapshot(id)
            .await
            .unwrap()
            .agents
            .len(),
        2
    );
    store
        .agent_log
        .update_one(
            doc! {"agent_id": terminal.id.as_str()},
            doc! {"$set": {"record.agent_id": "agt_foreign"}},
        )
        .await
        .unwrap();
    assert!(store.retained_runtime_recovery_snapshot(id).await.is_err());
    store
        .agent_log
        .update_one(
            doc! {"agent_id": terminal.id.as_str()},
            doc! {"$set": {"record.agent_id": terminal.id.as_str()}},
        )
        .await
        .unwrap();
    assert!(store.retained_runtime_recovery_snapshot(id).await.is_ok());
    let unchanged = store
        .agents
        .find_one(doc! {"_id": terminal.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!unchanged.contains_key("agent_log_integrity_quarantine_v1"));
    // A read-only assessment refuses missing parent events too.
    let event_request = fixture(&store, "sbx_retained_event_integrity").await;
    let id = &event_request.snapshot.sandbox.id;
    assert!(store.retained_runtime_recovery_snapshot(id).await.is_ok());
    store
        .sandboxes
        .update_one(
            doc! {"_id": id.as_str()},
            doc! {"$set": {"last_seq": 1_i64}},
        )
        .await
        .unwrap();
    assert!(
        store.retained_runtime_recovery_snapshot(id).await.is_err(),
        "missing Sandbox event cannot become an assessment"
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "retained_get_mongo_tests.rs"]
mod get;
