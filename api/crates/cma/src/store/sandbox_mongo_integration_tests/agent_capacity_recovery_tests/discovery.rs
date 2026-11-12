//! Bounded metadata enumeration cannot manufacture execution authority.
use super::*;
use crate::store::agent_capacity_recovery::{records, RecoveryService, RecoveryStream};
use layer2_core::sandbox::{
    agent_capacity_recovery::{ports::RecoveryError, state::RecoveryPhase},
    AgentWorkLease,
};
use mongodb::bson;
use records::test_support::NOW;

const MINUTE: &str = "2026-11-11T00:01:00Z";

mod bounds;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_discovery_keysets_are_bounded_separate_and_grant_no_authority() {
    let (client, name) = database("capacity_discovery").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let collection = records::collection(&store);
    crate::store::agent_log_segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let mut expected = Vec::new();
    for index in 0..69 {
        let mut work = records::test_support::work();
        work.request.agent_id = AgentId(format!("discovery-{index}"));
        work.target.agent_id = work.request.agent_id.clone();
        work.target.original_open.operation_id = format!("agent:{}:open:v1", work.request.agent_id);
        let key = work.request.key().unwrap();
        let mut version = 2_i64;
        if index < 65 {
            expected.push((key.clone(), work.request.clone()));
        } else if index < 67 {
            work.phase = RecoveryPhase::Attaching;
            work.revision += 1;
            work.attempts += 1;
            work.lease = Some(AgentWorkLease {
                holder: "expired".into(),
                fence: work.next_fence,
                expires_at: NOW.into(),
            });
            work.next_fence += 1;
        } else if index == 67 {
            work.ready_at = MINUTE.into();
        } else {
            version = 1;
        }
        // These rows have no matching source or Publication. They are only
        // candidates, even though discovery metadata is internally consistent.
        collection
            .insert_one(doc! {"_id": key, "format_version": version,
            "agent_id": work.request.agent_id.as_str(),
            "work": bson::serialize_to_bson(&work).unwrap(),
            "result": bson::serialize_to_bson(&records::test_support::stored().result).unwrap()})
            // Only the intentional obsolete-format row bypasses validation.
            .bypass_document_validation(version == 1)
            .await
            .unwrap();
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    let service = RecoveryService::new(store.clone(), transactions::access(&store));
    let first = service
        .discover(RecoveryStream::Admitted, None, NOW)
        .await
        .unwrap();
    assert_eq!(first.requests.len(), 32);
    let second = service
        .discover(RecoveryStream::Admitted, first.next.as_ref(), NOW)
        .await
        .unwrap();
    assert_eq!(second.requests.len(), 32);
    let third = service
        .discover(RecoveryStream::Admitted, second.next.as_ref(), NOW)
        .await
        .unwrap();
    assert_eq!(third.requests.len(), 1);
    assert!(third.next.is_none());
    let all: Vec<_> = first
        .requests
        .into_iter()
        .chain(second.requests)
        .chain(third.requests)
        .collect();
    assert_eq!(
        all,
        expected
            .into_iter()
            .map(|(_, request)| request)
            .collect::<Vec<_>>()
    );
    let expired = service
        .discover(RecoveryStream::ExpiredLease, None, NOW)
        .await
        .unwrap();
    assert_eq!(expired.requests.len(), 2);
    assert!(expired.next.is_none());
    let caller = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert!(matches!(
        service.claim(&caller, &all[0], "worker", NOW, MINUTE).await,
        Err(RecoveryError::Unauthorized)
    ));
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    collection
        .drop_index("agent_capacity_recovery_due")
        .await
        .unwrap();
    assert!(matches!(
        service.discover(RecoveryStream::Admitted, None, NOW).await,
        Err(RecoveryError::Unavailable)
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_restart_lookup_includes_completed_work_and_refuses_overflow() {
    let (client, name) = database("capacity_restart_candidates").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    crate::store::agent_log_segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let collection = records::collection(&store);
    let service = RecoveryService::new(store.clone(), transactions::access(&store));
    let agent_id = records::test_support::work().request.agent_id;
    assert!(service.discover_agent(&agent_id).await.unwrap().is_empty());
    let mut expected = Vec::new();
    for index in 0..33 {
        let mut stored = records::test_support::stored();
        stored.work.request.failure_seq += index;
        stored.result.through_seq = stored.work.request.failure_seq;
        stored.work.phase = RecoveryPhase::Completed;
        stored.work.revision = 3;
        stored.work.attempts = 1;
        stored.work.completed_at = Some(NOW.into());
        let key = stored.work.request.key().unwrap();
        // A schema-valid completed witness is still not a Publication or authority.
        collection
            .insert_one(doc! {"_id": &key, "format_version": 2_i64,
            "agent_id": agent_id.as_str(), "work": bson::serialize_to_bson(&stored.work).unwrap(),
            "result": bson::serialize_to_bson(&stored.result).unwrap()})
            .await
            .unwrap();
        expected.push((key, stored.work.request));
        if index == 31 {
            expected.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(
                service.discover_agent(&agent_id).await.unwrap(),
                expected
                    .iter()
                    .map(|(_, request)| request.clone())
                    .collect::<Vec<_>>()
            );
            assert!(service
                .discover(RecoveryStream::Admitted, None, NOW)
                .await
                .unwrap()
                .requests
                .is_empty());
            assert!(service
                .discover(RecoveryStream::ExpiredLease, None, NOW)
                .await
                .unwrap()
                .requests
                .is_empty());
            let caller = Caller::new(
                SubjectId::new("owner"),
                Authorization::new([Capability::UseOwnSandboxes]),
            );
            assert!(matches!(
                service
                    .claim(&caller, &expected[0].1, "restart", NOW, MINUTE)
                    .await,
                Err(RecoveryError::Unauthorized)
            ));
        }
    }
    assert!(matches!(
        service.discover_agent(&agent_id).await,
        Err(RecoveryError::Unavailable)
    ));
    assert!(service
        .discover_agent(&AgentId("different-agent".into()))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 33);
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 0);
    collection
        .drop_index("agent_capacity_recovery_agent")
        .await
        .unwrap();
    assert!(matches!(
        service.discover_agent(&agent_id).await,
        Err(RecoveryError::Unavailable)
    ));
    client.database(&name).drop().await.unwrap();
}
