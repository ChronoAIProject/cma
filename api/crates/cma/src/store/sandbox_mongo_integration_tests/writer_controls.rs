//! Process-separated reads and claims retain accepted control receipts.
use super::*;
use crate::store::writer_fixture::{client, database_name, emit, hash, input};
use serde_json::{json, Value};

async fn snapshot(store: &MongoSandboxStore, id: &AgentId) -> Value {
    let row = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let agent = store.get_agent(id).await.unwrap().unwrap();
    let mut parents = [
        store
            .sandboxes
            .find_one(doc! {"_id":agent.sandbox_id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        store
            .workspaces
            .find_one(doc! {"_id":agent.workspace_id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        store
            .worktrees
            .find_one(doc! {"_id":agent.worktree_id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    ];
    // Claim admission deliberately increments this one concurrency fence.
    // Preserve every other parent field, and assert this counter separately.
    let parent_claim_fence = parents[0]
        .remove("runtime_writer_claim_fence")
        .and_then(|value| value.as_i64())
        .unwrap();
    json!({"parent_claim_fence":parent_claim_fence,"id_hash":hash(id), "stop_receipt_hash":hash(&row.get(STOP_RECEIPT_FIELD)),
        "delete_receipt_hash":hash(&row.get(DELETE_RECEIPT_FIELD)),
        "parents_hash":hash(&parents), "status":agent.status, "step":agent.work.step,
        "revision":agent.revision,"lease_fence":agent.work.lease.as_ref().map(|v|v.fence)})
}

#[tokio::test]
#[ignore = "requires isolated cross-version writer fixture"]
async fn control_round() {
    let client = client().await;
    let database = database_name();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let inputs = client
        .database(&database)
        .collection::<Document>("writer_inputs");
    let action = input("CMA_WRITER_ACTION");
    let kind = input("CMA_WRITER_KIND");
    assert!(kind == "stop" || kind == "delete");
    if action == "seed" {
        let (_, _, mut agent) = claimed_agent_work_fixture(&store, "writer_control").await;
        if kind == "delete" {
            let revision = agent.revision;
            agent.work.schedule(AgentWorkStep::None, MINUTE.into());
            agent
                .transition(
                    AgentStatus::Stopped,
                    MINUTE.into(),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
            store.save_agent(revision, agent.clone()).await.unwrap();
        }
        let target = AgentControlTarget::from(&agent);
        inputs
            .insert_one(
                doc! {"_id":"target","record":mongodb::bson::serialize_to_bson(&target).unwrap()},
            )
            .await
            .unwrap();
        let result = if kind == "stop" {
            store
                .schedule_agent_stop(AgentStopRequest {
                    target,
                    reason: AgentStopReason::Owner,
                    requested_at: NOW.into(),
                })
                .await
                .unwrap()
        } else {
            store
                .schedule_agent_delete(AgentDeleteRequest {
                    target,
                    requested_at: TWO_MINUTES.into(),
                })
                .await
                .unwrap()
        };
        assert!(matches!(result, AgentControlResult::Scheduled(_)));
        emit(json!({"after":snapshot(&store,&agent.id).await}));
        return;
    }
    let row = inputs
        .find_one(doc! {"_id":"target"})
        .await
        .unwrap()
        .unwrap();
    let target: AgentControlTarget =
        mongodb::bson::deserialize_from_bson(row.get("record").unwrap().clone()).unwrap();
    let before = snapshot(&store, &target.agent_id).await;
    let replay = if kind == "stop" {
        store
            .schedule_agent_stop(AgentStopRequest {
                target: target.clone(),
                reason: AgentStopReason::Owner,
                requested_at: NOW.into(),
            })
            .await
            .unwrap()
    } else {
        store
            .schedule_agent_delete(AgentDeleteRequest {
                target: target.clone(),
                requested_at: TWO_MINUTES.into(),
            })
            .await
            .unwrap()
    };
    assert!(matches!(replay, AgentControlResult::Existing(_)));
    assert_eq!(snapshot(&store, &target.agent_id).await, before);
    let mut foreign = target.clone();
    foreign.owner = SubjectId::new("foreign-fixture");
    let refusal = if kind == "stop" {
        store
            .schedule_agent_stop(AgentStopRequest {
                target: foreign,
                reason: AgentStopReason::Owner,
                requested_at: NOW.into(),
            })
            .await
    } else {
        store
            .schedule_agent_delete(AgentDeleteRequest {
                target: foreign,
                requested_at: TWO_MINUTES.into(),
            })
            .await
    };
    assert_eq!(refusal, Err(AgentControlStoreError::NotFound));
    if action == "claim" {
        let now = input("CMA_WRITER_NOW");
        let until = input("CMA_WRITER_UNTIL");
        let claimed = store
            .claim_agent_work(
                &target.agent_id,
                "writer-control",
                now.clone(),
                until.clone(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(store
            .claim_agent_work(&target.agent_id, "foreign-worker", now, until)
            .await
            .unwrap()
            .is_none());
        assert!(claimed.revision > before["revision"].as_u64().unwrap());
    } else {
        assert_eq!(action, "read");
    }
    let after = snapshot(&store, &target.agent_id).await;
    assert_eq!(
        after["parent_claim_fence"].as_i64().unwrap(),
        before["parent_claim_fence"].as_i64().unwrap() + i64::from(action == "claim")
    );
    assert_eq!(
        after["parent_claim_fence"].as_i64().unwrap(),
        before["parent_claim_fence"].as_i64().unwrap() + i64::from(action == "claim")
    );
    for key in [
        "id_hash",
        "stop_receipt_hash",
        "delete_receipt_hash",
        "parents_hash",
        "status",
        "step",
    ] {
        assert_eq!(
            before[key], after[key],
            "pending custody field {key} must survive a writer claim"
        );
    }
    emit(json!({"before":before,"after":after,"accepted_replay":true,"foreign_refused":true}));
}
