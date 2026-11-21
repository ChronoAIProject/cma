use super::*;
use crate::sandbox::ownership_retry_hint::{
    AdvisoryDecision, HintWriteResult, OwnershipRetryHints,
};
use crate::store::agent_failure_pacing::ownership::{record::TestObservation, FIELD};
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use crate::store::MongoSandboxStore;
use layer2_core::agent_ownership::Authority;
use layer2_core::sandbox::store::{AgentStore, AgentWorkerStore};
use layer2_core::sandbox::{Agent, AgentWorkStep};
use mongodb::bson::{Bson, DateTime};
use std::time::SystemTime;

async fn claimed() -> (
    Client,
    String,
    MongoSandboxStore,
    Agent,
    Arc<Authority>,
    TestObservation,
) {
    let (client, name, store) = database().await;
    let (_, mut agent) = fixture(&store, "retry-race").await;
    let revision = agent.revision;
    agent.work.schedule(
        AgentWorkStep::PublishConnection,
        "2026-01-01T00:00:00Z".into(),
    );
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let store = store
        .enable_agent_ownership("manager-a".into())
        .await
        .unwrap()
        .with_ownership_retry_hints(true);
    let manager = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        member("manager-a"),
    )
    .await
    .unwrap();
    let authority = manager.acquire(&agent.id).await.unwrap().unwrap();
    let now = SystemTime::now();
    let agent = authority
        .run(store.claim_agent_work(
            &agent.id,
            "worker",
            layer2_core::rfc3339_at(now),
            layer2_core::rfc3339_at(now + Duration::from_secs(600)),
        ))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let captured = TestObservation::capture(&store, &agent, &authority.token).await;
    (client, name, store, agent, authority, captured)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_snapshot_read_conflicts_with_competing_runtime_and_admission_writes()
{
    for (collection, field, value) in [
        (
            "sandbox",
            "record.runtime.runtime_id",
            Bson::String("new-runtime".into()),
        ),
        (
            "sandbox",
            "record.runtime.boot_id",
            Bson::String("new-boot".into()),
        ),
        (
            "sandbox",
            "record.runtime.runtime_version",
            Bson::String("new-version".into()),
        ),
        (
            "sandbox",
            "record.runtime.provision_generation",
            Bson::Int64(2),
        ),
        (
            "sandbox",
            "record.lifecycle.runtime_generation_high_water",
            Bson::Int64(2),
        ),
        ("agent", "agent_command_admission_fence", Bson::Int64(1)),
        (
            "agent",
            "record.requested_stop_reason",
            Bson::String("sandbox_retired".into()),
        ),
        (
            "agent",
            "record.deleted_at",
            Bson::String("2026-11-18T00:00:00Z".into()),
        ),
    ] {
        let (client, name, store, agent, _, mut observed) = claimed().await;
        let gate = observed.pause_at(if collection == "sandbox" {
            "parent_read"
        } else {
            "agent_read"
        });
        let writer = tokio::spawn(async move { observed.write().await });
        tokio::time::timeout(Duration::from_secs(1), gate.arrived.notified())
            .await
            .unwrap();
        let (rows, id) = if collection == "sandbox" {
            (&store.sandboxes, agent.sandbox_id.as_str())
        } else {
            (&store.agents, agent.id.as_str())
        };
        // This commits after the transaction's snapshot read but before its CAS.
        rows.update_one(doc! {"_id": id}, doc! {"$set": {field: value}})
            .await
            .unwrap();
        gate.resume.notify_one();
        assert_eq!(writer.await.unwrap(), HintWriteResult::Stale, "{field}");
        assert!(!store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .contains_key(FIELD));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_original_owner_cannot_write_after_takeover_expiry_drain_or_handoff() {
    for action in ["takeover", "expiry", "drain", "handoff"] {
        let (client, name, store, agent, authority, mut observed) = claimed().await;
        let gate = observed.pause_at("before_fence");
        let writer = tokio::spawn(async move { observed.write().await });
        tokio::time::timeout(Duration::from_secs(1), gate.arrived.notified())
            .await
            .unwrap();
        let ownership = store.agent_ownership.as_ref().unwrap();
        match action {
            "drain" => {
                ownership.heartbeat("manager-a", true).await.unwrap();
            }
            "handoff" => {
                ownership.release(&authority.token).await.unwrap();
            }
            "expiry" | "takeover" => {
                ownership
                    .owners
                    .update_one(
                        doc! {"_id": agent.id.as_str()},
                        doc! {"$set": {"expires_at": DateTime::from_millis(0)}},
                    )
                    .await
                    .unwrap();
                if action == "takeover" {
                    ownership.heartbeat("manager-a", true).await.unwrap();
                    let manager =
                        OwnershipCoordinator::register(ownership.clone(), member("manager-b"))
                            .await
                            .unwrap();
                    let next = manager.acquire(&agent.id).await.unwrap().unwrap();
                    assert!(next.token.epoch > authority.token.epoch);
                }
            }
            _ => unreachable!(),
        }
        gate.resume.notify_one();
        assert_eq!(writer.await.unwrap(), HintWriteResult::Stale, "{action}");
        assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Proceed);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_transaction_really_writes_member_owner_and_parent_fences() {
    let (client, name, store, agent, authority, observed) = claimed().await;
    let ownership = store.agent_ownership.as_ref().unwrap();
    let mut before = Vec::new();
    for (rows, id, field) in [
        (
            &ownership.members,
            authority.token.instance_id.as_str(),
            "effect_revision",
        ),
        (&ownership.owners, agent.id.as_str(), "effect_revision"),
        (
            &store.sandboxes,
            agent.sandbox_id.as_str(),
            "runtime_writer_claim_fence",
        ),
    ] {
        before.push(
            rows.find_one(doc! {"_id": id})
                .await
                .unwrap()
                .unwrap()
                .get_i64(field)
                .unwrap_or(0),
        );
    }
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    for ((rows, id, field), previous) in [
        (
            &ownership.members,
            authority.token.instance_id.as_str(),
            "effect_revision",
        ),
        (&ownership.owners, agent.id.as_str(), "effect_revision"),
        (
            &store.sandboxes,
            agent.sandbox_id.as_str(),
            "runtime_writer_claim_fence",
        ),
    ]
    .into_iter()
    .zip(before)
    {
        assert_eq!(
            rows.find_one(doc! {"_id": id})
                .await
                .unwrap()
                .unwrap()
                .get_i64(field)
                .unwrap(),
            previous + 1
        );
    }
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_fences_conflict_with_concurrent_authority_and_runtime_writes() {
    let (client, name, store, agent, authority, mut observed) = claimed().await;
    let gate = observed.pause_at("agent_written");
    let writer = tokio::spawn(async move { observed.write().await });
    tokio::time::timeout(Duration::from_secs(1), gate.arrived.notified())
        .await
        .unwrap();
    let ownership = store.agent_ownership.as_ref().unwrap();
    for (rows, id, field) in [
        (
            &ownership.members,
            authority.token.instance_id.as_str(),
            "effect_revision",
        ),
        (&ownership.owners, agent.id.as_str(), "effect_revision"),
        (
            &store.sandboxes,
            agent.sandbox_id.as_str(),
            "runtime_writer_claim_fence",
        ),
        (
            &store.agents,
            agent.id.as_str(),
            "agent_command_admission_fence",
        ),
    ] {
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let conflict = rows
            .update_one(doc! {"_id": id}, doc! {"$inc": {field: 1_i64}})
            .session(&mut session)
            .await
            .expect_err("hint must own this row's transaction write lock");
        assert!(
            conflict.contains_label("TransientTransactionError"),
            "{field}: {conflict}"
        );
        session.abort_transaction().await.unwrap();
    }
    gate.resume.notify_one();
    assert_eq!(writer.await.unwrap(), HintWriteResult::Recorded);
    assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Hold);
    client.database(&name).drop().await.unwrap();
}
