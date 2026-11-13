use super::{
    partition::{PartitionFence, PartitionLease},
    partition_model::*,
};
use crate::ingress::{
    routing_plan::fixture,
    routing_store::{test_server::Server, AdmittedPlan, Candidate},
};

pub(super) async fn plan() -> (Server, AdmittedPlan) {
    let server = Server::start().await;
    let request = fixture::request(fixture::targets, fixture::message("partition fixture"));
    let candidate = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"fixture",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let plan = server.store().admit(&candidate).await.unwrap();
    (server, plan)
}

#[tokio::test]
async fn canonical_conversation_key_excludes_event_target_and_body_but_separates_authority() {
    let (_server, plan) = plan().await;
    let target = &plan.targets[0];
    let key = partition_id("deployment", &plan, target);
    let mut replay = plan.clone();
    replay.identity.event_id = cmaeg_core::ids::EventId::new("another-event").unwrap();
    replay.request_hash = "b".repeat(64);
    replay.source_body = b"changed-body".to_vec();
    replay.targets[0].id = uuid::Uuid::new_v4().to_string();
    assert_eq!(key, partition_id("deployment", &replay, &replay.targets[0]));
    assert_ne!(key, partition_id("another-deployment", &plan, target));
    replay.identity.owner.push_str("-other");
    assert_ne!(key, partition_id("deployment", &replay, target));
    let mut other = target.clone();
    other.launch.event.partition_key.push_str("/other-thread");
    assert_ne!(key, partition_id("deployment", &plan, &other));
    other = target.clone();
    other.launch.consumer = cmaeg_core::ids::ConsumerId::parse("csm_ffffffffffff").unwrap();
    assert_ne!(key, partition_id("deployment", &plan, &other));
}

#[tokio::test]
async fn retained_completion_replays_only_the_exact_original_admission() {
    let (_server, plan) = plan().await;
    let expected = Item::new("partition".into(), &plan, &plan.targets[0]);
    let mut retained = expected.clone();
    assert!(retained.same_admission(&expected));
    retained.done = true;
    assert!(retained.same_admission(&expected));
    for field in ["partition", "target", "request_hash", "owner", "retention"] {
        let mut changed = retained.clone();
        match field {
            "partition" => changed.partition.push('x'),
            "target" => changed.target.push('x'),
            "request_hash" => changed.request_hash.push('x'),
            "owner" => changed.identity.owner.push('x'),
            _ => changed.retained_until += 1,
        }
        assert!(!changed.same_admission(&expected));
    }
}

#[tokio::test(start_paused = true)]
async fn a_captured_token_cannot_adopt_successor_epoch_front_or_later_deadline() {
    use std::time::Duration;
    let target = uuid::Uuid::new_v4().to_string();
    let authority = Authority {
        instance: "a".repeat(64),
        claim: uuid::Uuid::new_v4().to_string(),
        epoch: 7,
        until_ms: 15_000,
    };
    let mut head = Head::empty(
        uuid::Uuid::new_v4().to_string(),
        "deployment".into(),
        "owner".into(),
    );
    head.epoch = 7;
    head.authority = Some(authority.clone());
    head.queue.push_back(target.clone());
    assert!(head
        .validate(&head.id, &head.deployment, &head.owner)
        .is_ok());
    assert!(head.matches(&target, &authority));
    head.authority.as_mut().unwrap().until_ms = 18_000;
    assert!(head.matches(&target, &authority));
    let lease = PartitionLease {
        id: head.id.clone(),
        owner: head.owner.clone(),
        target: target.clone(),
        work_id: target.clone(),
        token: authority.clone(),
        deadline: tokio::time::Instant::now() + Duration::from_secs(15),
    };
    let captured = lease.clone();
    let fence = PartitionFence {
        instance: authority.instance.clone(),
        claim: authority.claim.clone(),
        epoch: authority.epoch,
        until_ms: authority.until_ms,
    };
    assert!(captured.matches(&fence));
    let mut successor = fence.clone();
    successor.epoch += 1;
    assert!(!captured.matches(&successor));
    successor = fence.clone();
    successor.instance = "b".repeat(64);
    assert!(!captured.matches(&successor));
    head.authority.as_mut().unwrap().epoch += 1;
    assert!(!head.matches(&target, &authority));
    head.authority = Some(authority.clone());
    head.queue.push_front(uuid::Uuid::new_v4().to_string());
    assert!(!head.matches(&target, &authority));
    tokio::time::advance(Duration::from_secs(15)).await;
    assert!(!captured.matches(&fence));
    assert!(!lease.live());
}

#[test]
fn corrupt_queue_authority_fails_closed() {
    let id = uuid::Uuid::new_v4().to_string();
    let mut head = Head::empty(id.clone(), "deployment".into(), "owner".into());
    assert!(head.validate(&id, "deployment", "owner").is_ok());
    assert!(head.validate(&id, "deployment", "wrong-owner").is_err());
    head.queue.push_back(id.clone());
    head.queue.push_back(id.clone());
    assert!(head.validate(&id, "deployment", "owner").is_err());
    head.queue.pop_back();
    head.authority = Some(Authority {
        instance: "a".repeat(64),
        claim: uuid::Uuid::new_v4().to_string(),
        epoch: 1,
        until_ms: 15_000,
    });
    assert!(head.validate(&id, "deployment", "owner").is_err());
    head.epoch = 1;
    assert!(head.validate(&id, "deployment", "owner").is_ok());
    head.queue.clear();
    assert!(head.validate(&id, "deployment", "owner").is_err());
}
