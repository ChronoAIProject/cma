use super::ownership_retry_wake_tests::fixture;
use super::*;
use crate::sandbox::ownership_retry_hint::{
    AdvisoryDecision, HintWriteResult, OwnershipRetryHints,
};
use crate::store::agent_failure_pacing::{
    discovery,
    ownership::{record::TestObservation, FIELD as HINT},
};
use crate::store::agents::{agent_replacement_fields, read_agent};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_unchanged_predecessor_decoder_and_updates_accept_both_envelopes() {
    // These are the actual predecessor codec/update functions, unchanged by M2.
    // No copied or relaxed domain schema participates in this contract.
    for segmented in [false, true] {
        let (client, name, store, agent, authority, _) = fixture("retry_compatibility").await;
        if segmented {
            use crate::store::agent_log_segments::{incarnation, FORMAT};
            store.agents.update_one(doc! {"_id": agent.id.as_str()}, doc! {"$set": {
                "document_schema_version": 2_i64, "agent_log_format": FORMAT,
                "agent_log_incarnation": incarnation(&agent), "agent_log_head_revision": 1_i64,
            }}).await.unwrap();
        }
        let observed = TestObservation::capture(&store, &agent, &authority.token).await;
        assert_eq!(observed.write().await, HintWriteResult::Recorded);
        let original = store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_agent(&original).unwrap(), agent);
        assert!(
            mongodb::bson::serialize_to_vec(original.get_document(HINT).unwrap())
                .unwrap()
                .len()
                <= 4096
        );
        for retain in [false, true] {
            store
                .agents
                .replace_one(doc! {"_id": agent.id.as_str()}, original.clone())
                .await
                .unwrap();
            let mut next = agent.clone();
            next.revision += 1;
            let mut update = doc! {"$set": agent_replacement_fields(&next).unwrap()};
            if !retain {
                update.insert("$unset", doc! {HINT: ""});
            }
            store
                .agents
                .update_one(
                    doc! {"_id": agent.id.as_str(), "revision": agent.revision as i64},
                    update,
                )
                .await
                .unwrap();
            assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(next));
            assert_eq!(
                store.preflight(&agent.id).await,
                if retain {
                    AdvisoryDecision::Hold
                } else {
                    AdvisoryDecision::Proceed
                }
            );
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_flag_off_and_missing_ownership_skip_reads_and_writes() {
    let (client, name, store, agent, authority, observed) = fixture("retry_flag_matrix").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    let original = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let absent = MongoSandboxStore::open(&client, &name).await.unwrap();
    let without_owner = absent.clone().with_ownership_retry_hints(true);
    let explicit_off = store.clone().with_ownership_retry_hints(false);
    let baseline = discovery::pipeline(&absent, NOW, 1);
    for inactive in [absent, without_owner, explicit_off] {
        assert_eq!(discovery::pipeline(&inactive, NOW, 1), baseline);
        assert_eq!(
            inactive.preflight(&agent.id).await,
            AdvisoryDecision::Proceed
        );
        assert!(inactive.capture(&agent, &authority.token).await.is_none());
        assert!(Arc::new(inactive).ownership_retry_hints().is_none());
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap(),
            original
        );
    }
    assert!(Arc::new(store.clone()).ownership_retry_hints().is_some());
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {
                HINT: "malformed-advisory", "record.status": "malformed-authority",
            }},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.get_agent(&agent.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(
        store.preflight(&agent.id).await,
        AdvisoryDecision::Unavailable
    );
    assert!(matches!(
        store
            .claim_agent_work(&agent.id, "worker", NOW.into(), stamp(600))
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_never_blocks_real_stop_admission_or_its_worker_claim() {
    use layer2_core::sandbox::store::{AgentControlStore, AgentControlTarget, AgentStopRequest};
    use layer2_core::sandbox::AgentStopReason;
    for hint_first in [true, false] {
        let (client, name, store, agent, authority, observed) = fixture("retry_stop").await;
        if hint_first {
            assert_eq!(observed.write().await, HintWriteResult::Recorded);
            assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Hold);
        }
        let now = SystemTime::now();
        store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason: AgentStopReason::Owner,
                requested_at: layer2_core::rfc3339_at(now),
            })
            .await
            .unwrap();
        assert_eq!(observed.write().await, HintWriteResult::Stale);
        assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Proceed);
        let stopped = authority
            .run(store.claim_agent_work(
                &agent.id,
                "stop-worker",
                layer2_core::rfc3339_at(now),
                layer2_core::rfc3339_at(now + Duration::from_secs(600)),
            ))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(stopped.requested_stop_reason, Some(AgentStopReason::Owner));
        assert_eq!(stopped.work.step, AgentWorkStep::FenceInteraction);
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap()
                .contains_key(HINT),
            hint_first
        );
        client.database(&name).drop().await.unwrap();
    }
}
