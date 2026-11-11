use super::*;
use layer2_core::sandbox::store::AgentRetryObservation;
use std::time::{Duration, SystemTime};

const PARENT: &str = "agent_parent_not_ready";
const FIELD: &str = "worker_failure_pacing_v1";

mod segmented;

fn stamp(seconds: u64) -> String {
    layer2_core::rfc3339_at(
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(layer2_core::seconds_from_rfc3339(NOW).unwrap() + seconds),
    )
}

async fn failure(store: &MongoSandboxStore, id: &AgentId, seconds: u64, class: &str) {
    let current = store
        .claim_agent_work(id, "worker", stamp(seconds), stamp(seconds + 600))
        .await
        .unwrap()
        .unwrap();
    let observed = AgentRetryObservation {
        agent_revision: current.revision,
        command_admission_generation: store
            .agent_command_admission_generation(id)
            .await
            .unwrap()
            .unwrap(),
        sandbox_revision: store
            .get_sandbox(&current.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        workspace_revision: store
            .get_workspace(&current.workspace_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        worktree_revision: store
            .get_worktree(&current.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
    };
    let fence = current.work.lease.as_ref().unwrap().fence;
    let mut next = current.clone();
    next.work
        .retry("worker", fence, stamp(seconds + 30))
        .unwrap();
    next.revision += 1;
    next.updated_at = stamp(seconds);
    next.conversation.last_seq += 1;
    store
        .commit_agent_work_retry(
            AgentWorkMutation {
                expected_agent_revision: current.revision,
                holder: "worker".into(),
                fence,
                agent: next,
                appended_at: stamp(seconds),
                worktree: None,
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::WorkStepFailed {
                    step: AgentWorkStep::PublishConnection,
                    failure: TypedFailure {
                        class: class.into(),
                        detail: "read-only parent observation".into(),
                    },
                }],
            },
            observed,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn parent_pacing_survives_restart_wakes_progress_and_preserves_inputs() {
    let (client, name) = database("capacity_parent_pacing").await;
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = ready_conversation_fixture(&store, "parent").await;
    for at in [0, 30, 90, 210, 450] {
        failure(&store, &original.id, at, PARENT).await;
    }
    store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(store
        .due_agent_work(&stamp(480), 1)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .claim_agent_work(&original.id, "other", stamp(480), stamp(1000))
        .await
        .unwrap()
        .is_none());
    let row = store
        .agents
        .find_one(doc! {"_id": original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_document(FIELD).unwrap().get_str("until").unwrap(),
        stamp(750)
    );
    assert!(!row.get_document("record").unwrap().contains_key(FIELD));
    let saved = store.get_agent(&original.id).await.unwrap().unwrap();
    assert_eq!(saved.work.receipts, original.work.receipts);
    assert_eq!(
        saved.conversation.first_message,
        original.conversation.first_message
    );
    assert_eq!(
        saved.conversation.queued_messages,
        original.conversation.queued_messages
    );
    assert_eq!(
        saved.conversation.last_seq,
        original.conversation.last_seq + 5
    );
    store
        .sandboxes
        .update_one(
            doc! {"_id": original.sandbox_id.as_str()},
            doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(481), 1).await.unwrap(),
        vec![original.id.clone()]
    );
    failure(&store, &original.id, 481, PARENT).await;
    let row = store
        .agents
        .find_one(doc! {"_id": original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_document(FIELD).unwrap().get_str("until").unwrap(),
        stamp(511)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn parent_pacing_class_changes_reset_and_owner_stop_wakes() {
    let (client, name) = database("capacity_parent_stop").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "stop").await;
    failure(&store, &agent.id, 0, "agent_runtime_unavailable").await;
    failure(&store, &agent.id, 30, "agent_runtime_unavailable").await;
    failure(&store, &agent.id, 90, PARENT).await;
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_document(FIELD).unwrap().get_str("until").unwrap(),
        stamp(120)
    );
    failure(&store, &agent.id, 120, PARENT).await;
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&current),
            reason: AgentStopReason::Owner,
            requested_at: stamp(121),
        })
        .await
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(121), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    let stopped = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stopped.requested_stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(stopped.work.step, AgentWorkStep::FenceInteraction);
    client.database(&name).drop().await.unwrap();
}
