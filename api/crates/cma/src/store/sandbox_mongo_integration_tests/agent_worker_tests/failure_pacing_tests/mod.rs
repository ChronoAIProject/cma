//! Real persistence/claim/publication contracts, with a deterministic logical clock.
use super::*;
use layer2_core::sandbox::store::AgentRetryObservation;
use std::time::{Duration, SystemTime};

mod admission_tests;
mod helpers;
mod wake_tests;
mod worker_tests;
use helpers::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_survives_restart_and_reduces_complete_scans() {
    crate::store::agent_log_telemetry::isolated_test(async {
        let (client, name) = database("failure_pacing_counts").await;
        let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let agent = ready_conversation_fixture(&store, "paced",).await;
        let observed = observation(&store, &agent).await;
        let before = scans();
        let mut attempts = Vec::new();
        for seconds in (0..=3600).step_by(30) {
            // Reopen while the first extended hold is pending; no local state
            // survives to suppress the next retry or reconstruct its streak.
            if seconds == 60 {
                store = MongoSandboxStore::open(&client, &name).await.unwrap();
            }
            let ids = store.due_agent_work(&stamp(seconds), 1).await.unwrap();
            if ids.contains(&agent.id) {
                fail(&store, &agent.id, seconds, Some(observed.clone()), CLASS).await;
                attempts.push(seconds);
            } else {
                assert!(store.claim_agent_work(&agent.id, "other-worker", stamp(seconds), stamp(seconds + 600)).await.unwrap().is_none());
            }
        }
        assert_eq!(&attempts[..5], &[0, 30, 90, 210, 450]);
        assert!(attempts.windows(2).all(|pair| pair[1] - pair[0] <= 300));
        let paced_scans = scans() - before;
        assert_eq!(paced_scans, attempts.len() as u64);
        assert_eq!(paced_scans, 15);
        let saved = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
        assert_eq!(saved.work.attempts, paced_scans as u32);
        assert_eq!(saved.conversation.last_seq, agent.conversation.last_seq + paced_scans);
        assert_eq!(saved.work.receipts, agent.work.receipts);
        assert_eq!(saved.work.cleanup_debt, agent.work.cleanup_debt);

        let ordinary = ready_conversation_fixture(&store, "ordinary").await;
        let before = scans();
        for seconds in (0..=3600).step_by(30) {
            // Same typed failure without a read-only observation: ambiguous
            // operation outcomes must retain the ordinary retry path.
            fail(&store, &ordinary.id, seconds, None, CLASS).await;
        }
        let ordinary_scans = scans() - before;
        assert_eq!(ordinary_scans, 121);
        println!("FAILURE_PACING scans_before={ordinary_scans} scans_after={paced_scans} horizon_seconds=3600 maximum_retry_seconds=300");
        client.database(&name).drop().await.unwrap();
    }).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_does_not_starve_first_work_or_expired_fallback() {
    let (client, name) = database("failure_pacing_fairness").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let held = ready_conversation_fixture(&store, "a_held").await;
    let observed = observation(&store, &held).await;
    for second in [0, 30, 90, 210, 450] {
        fail(&store, &held.id, second, Some(observed.clone()), CLASS).await;
    }
    let fresh = ready_conversation_fixture(&store, "z_fresh").await;
    assert!(held.id.as_str() < fresh.id.as_str());
    // Held work is filtered before LIMIT 1, even when it sorts first.
    assert_eq!(
        store.due_agent_work(&stamp(451), 1).await.unwrap(),
        vec![fresh.id.clone()]
    );
    let claimed = store
        .claim_agent_work(&fresh.id, "worker", stamp(451), stamp(1051))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.attempts, 0);
    assert!(store
        .due_agent_work(&stamp(749), 1)
        .await
        .unwrap()
        .is_empty());
    // No notification or process-local state is needed at the fallback bound.
    assert_eq!(
        store.due_agent_work(&stamp(750), 1).await.unwrap(),
        vec![held.id]
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_class_change_and_missing_hint_keep_ordinary_retries() {
    let (client, name) = database("failure_pacing_classes").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "classes").await;
    let observed = observation(&store, &agent).await;
    fail(&store, &agent.id, 0, Some(observed.clone()), CLASS).await;
    fail(&store, &agent.id, 30, Some(observed.clone()), CLASS).await;
    fail(
        &store,
        &agent.id,
        90,
        Some(observed.clone()),
        "agent_runtime_attachment_conflict",
    )
    .await;
    assert_eq!(
        store.due_agent_work(&stamp(120), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    fail(&store, &agent.id, 120, Some(observed.clone()), CLASS).await;
    assert_eq!(until(&store, &agent.id).await, stamp(150));
    fail(&store, &agent.id, 150, Some(observed), CLASS).await;
    assert_eq!(until(&store, &agent.id).await, stamp(210));
    // A rollback reader or a crash between publication and hint CAS loses only
    // the optimization; the unmodified pending-work record is still claimable.
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$unset": {FIELD: ""}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(180), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    assert!(store
        .claim_agent_work(&agent.id, "rollback-worker", stamp(180), stamp(181))
        .await
        .unwrap()
        .is_some());
    assert!(store
        .claim_agent_work(&agent.id, "takeover-worker", stamp(180), stamp(190))
        .await
        .unwrap()
        .is_none());
    assert!(store
        .claim_agent_work(&agent.id, "takeover-worker", stamp(181), stamp(190))
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}
