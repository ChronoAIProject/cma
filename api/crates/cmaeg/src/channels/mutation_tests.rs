//! Identical race assertions over HTTP CAS and authenticated native Mongo.
use super::{
    management,
    mutation_fixture::{owner, wait, Fixture},
};
use std::sync::atomic::Ordering;

async fn race(native: bool, delete: bool) {
    let fixture = Fixture::new(native, true).await;
    let first = fixture.state().await;
    let second = fixture.state().await;
    let body = fixture.body();
    let creator = owner("alice", false);
    let created = management::create(
        &first,
        &creator,
        serde_json::from_value(body.clone()).unwrap(),
    )
    .await
    .unwrap();
    fixture.peer.pause.store(true, Ordering::SeqCst);
    let state = first.clone();
    let id = created.channel_id.clone();
    let request = fixture.update(&body, created.version, "First");
    let task = tokio::spawn(async move {
        management::update(&state, &owner("alice", false), &id, request).await
    });
    wait(&fixture.peer.started).await;
    if delete {
        assert!(
            management::delete(&second, &creator, &created.channel_id, created.version)
                .await
                .is_err()
        );
    } else {
        assert!(management::update(
            &second,
            &creator,
            &created.channel_id,
            fixture.update(&body, created.version, "Second")
        )
        .await
        .is_err());
    }
    let held = management::load(&second, &creator, &created.channel_id)
        .await
        .unwrap();
    assert!(held.definition.mutation.is_some());
    assert!(management::rekey(&second, &created.channel_id)
        .await
        .is_err());
    // Even a freshly read version grants no successor or deletion authority.
    assert!(
        management::delete(&second, &creator, &created.channel_id, held.record.version)
            .await
            .is_err()
    );
    assert!(management::update(
        &second,
        &creator,
        &created.channel_id,
        fixture.update(&body, held.record.version, "Overtake")
    )
    .await
    .is_err());
    assert_eq!(*fixture.peer.calls.lock().unwrap(), ["PUT", "PUT"]);
    fixture.peer.resume.notify_one();
    task.await.unwrap().unwrap();
    let winner = management::load(&second, &creator, &created.channel_id)
        .await
        .unwrap();
    assert_eq!(winner.definition.name, "First");
    assert!(winner.definition.mutation.is_none());
    let consumer = winner.definition.consumer(None).unwrap();
    assert_eq!(
        fixture.peer.bound.lock().unwrap()[consumer.provider.binding_id.as_str()]["profile_digest"],
        consumer.profile_digest
    );
    management::delete(
        &second,
        &creator,
        &created.channel_id,
        winner.record.version,
    )
    .await
    .unwrap();
    assert!(fixture.peer.bound.lock().unwrap().is_empty());
    fixture.cleanup().await;
}
#[tokio::test]
async fn http_update_races_cannot_overtake_provider_publication() {
    for delete in [false, true] {
        race(false, delete).await;
    }
}
#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn native_update_races_cannot_overtake_provider_publication() {
    for delete in [false, true] {
        race(true, delete).await;
    }
}

async fn unknown(native: bool) {
    let fixture = Fixture::new(native, true).await;
    let state = fixture.state().await;
    let body = fixture.body();
    let creator = owner("alice", false);
    let created = management::create(
        &state,
        &creator,
        serde_json::from_value(body.clone()).unwrap(),
    )
    .await
    .unwrap();
    fixture.peer.pause.store(true, Ordering::SeqCst);
    let first = state.clone();
    let id = created.channel_id.clone();
    let request = fixture.update(&body, created.version, "Late");
    let task = tokio::spawn(async move {
        management::update(&first, &owner("alice", false), &id, request).await
    });
    wait(&fixture.peer.started).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let other = fixture.state().await;
    let held = management::load(&other, &creator, &created.channel_id)
        .await
        .unwrap();
    let nonce = held.definition.mutation.as_ref().unwrap().nonce.clone();
    assert!(
        management::delete(&other, &creator, &created.channel_id, held.record.version)
            .await
            .is_err()
    );
    fixture.peer.resume.notify_one();
    wait(&fixture.peer.finished).await;
    assert_eq!(
        management::load(&other, &creator, &created.channel_id)
            .await
            .unwrap()
            .definition
            .mutation
            .unwrap()
            .nonce,
        nonce
    );
    assert!(management::update(
        &other,
        &creator,
        &created.channel_id,
        fixture.update(&body, held.record.version, "Replacement")
    )
    .await
    .is_err());
    assert_eq!(*fixture.peer.calls.lock().unwrap(), ["PUT", "PUT"]);
    fixture.cleanup().await;
}
#[tokio::test]
async fn http_cancelled_mutation_retains_unknown_custody() {
    unknown(false).await;
}
#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn native_cancelled_mutation_retains_unknown_custody() {
    unknown(true).await;
}

#[tokio::test]
async fn exact_lost_reservation_and_publication_acks_do_not_resend_provider() {
    let fixture = Fixture::new(false, true).await;
    let state = fixture.state().await;
    let body = fixture.body();
    let creator = owner("alice", false);
    let created = management::create(
        &state,
        &creator,
        serde_json::from_value(body.clone()).unwrap(),
    )
    .await
    .unwrap();
    fixture
        .peer
        .lose_ack
        .store(created.version, Ordering::SeqCst);
    let next = management::update(
        &state,
        &creator,
        &created.channel_id,
        fixture.update(&body, created.version, "Reserved"),
    )
    .await
    .unwrap();
    // Reserve, effect admission, confirmed checkpoint, then publication: lose the final acknowledgement.
    fixture
        .peer
        .lose_ack
        .store(next.record.version + 3, Ordering::SeqCst);
    let next = management::update(
        &state,
        &creator,
        &created.channel_id,
        fixture.update(&body, next.record.version, "Published"),
    )
    .await
    .unwrap();
    assert_eq!(next.definition.name, "Published");
    assert_eq!(*fixture.peer.calls.lock().unwrap(), ["PUT", "PUT", "PUT"]);
    fixture.cleanup().await;
}

#[tokio::test]
async fn routing_and_audit_cannot_bypass_held_custody_or_owner_kind() {
    use super::{
        audit::AuditReport,
        audit_routing_clear::{clear, Clearing},
        routing_update,
    };
    use std::collections::BTreeSet;
    let fixture = Fixture::new(false, true).await;
    let first = fixture.state().await;
    let other = fixture.state().await;
    let body = fixture.body();
    let creator = owner("alice", false);
    let created = management::create(
        &first,
        &creator,
        serde_json::from_value(body.clone()).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        management::load(&other, &owner("alice", true), &created.channel_id)
            .await
            .is_err()
    );
    assert!(
        management::load(&other, &owner("bob", false), &created.channel_id)
            .await
            .is_err()
    );
    fixture.peer.pause.store(true, Ordering::SeqCst);
    let state = first.clone();
    let id = created.channel_id.clone();
    let request = fixture.update(&body, created.version, "Held");
    let task = tokio::spawn(async move {
        management::update(&state, &owner("alice", false), &id, request).await
    });
    wait(&fixture.peer.started).await;
    let held = management::load(&other, &creator, &created.channel_id)
        .await
        .unwrap();
    let request=serde_json::from_value(serde_json::json!({"expected_version":held.record.version,"routing":{"version":1,"default_consumer":{"name":"lead"},"default_targets":["default"]}})).unwrap();
    assert!(
        routing_update::update(&other, &creator, &created.channel_id, request)
            .await
            .is_err()
    );
    let non_human = BTreeSet::new();
    let clearing = Clearing {
        records: &other.records,
        keys: &fixture.keys,
        providers: Some(&other.providers),
        non_human: &non_human,
        allow_default_widening: false,
    };
    let head = crate::records::RecordHead {
        record_id: created.record_id.to_string(),
        owner: "alice".into(),
        version: held.record.version,
    };
    let outcome = clear(
        &clearing,
        &head,
        held.definition.clone(),
        held.record.version,
        &mut AuditReport::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.action,
        super::audit_routing::RoutingAuditAction::Conflict
    );
    assert!(held.definition.to_public_json().get("mutation").is_none());
    let reopened: cmaeg_core::channel::ChannelDefinition =
        serde_json::from_value(held.definition.to_blob_json()).unwrap();
    assert_eq!(reopened.mutation, held.definition.mutation);
    fixture.peer.resume.notify_one();
    task.await.unwrap().unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
async fn stable_creation_excludes_retry_delete_and_compensation() {
    let fixture = Fixture::new(false, true).await;
    let first = fixture.state().await;
    let other = fixture.state().await;
    let body = fixture.body();
    let task_body = body.clone();
    fixture.peer.pause.store(true, Ordering::SeqCst);
    let task = tokio::spawn(async move {
        management::create(
            &first,
            &owner("alice", false),
            serde_json::from_value(task_body).unwrap(),
        )
        .await
    });
    wait(&fixture.peer.started).await;
    assert!(management::create(
        &other,
        &owner("alice", false),
        serde_json::from_value(body.clone()).unwrap()
    )
    .await
    .is_err());
    let retained = super::creation::load_record(
        &other,
        &owner("alice", false),
        &body["record_id"].as_str().unwrap().parse().unwrap(),
    )
    .await
    .unwrap();
    assert!(management::delete(
        &other,
        &owner("alice", false),
        &retained.channel_id,
        retained.record.version
    )
    .await
    .is_err());
    assert_eq!(*fixture.peer.calls.lock().unwrap(), ["PUT"]);
    fixture.peer.resume.notify_one();
    task.await.unwrap().unwrap();
    management::create(
        &other,
        &owner("alice", false),
        serde_json::from_value(body).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(*fixture.peer.calls.lock().unwrap(), ["PUT"]);
    fixture.cleanup().await;
}

#[tokio::test]
async fn exact_safe_retry_fences_the_previous_claimant_after_writer_closure() {
    use super::mutation::{Intent, Reservation};
    use cmaeg_core::channel::ChannelMutationKind;
    let fixture = Fixture::new(false, true).await;
    let first = fixture.state().await;
    let mut other = fixture.state().await;
    other.records = std::sync::Arc::new(
        (*other.records)
            .clone()
            .with_channel_mutation_custody(false),
    );
    let creator = owner("alice", false);
    let body = fixture.body();
    let created = management::create(&first, &creator, serde_json::from_value(body).unwrap())
        .await
        .unwrap();
    let intent = || Intent {
        operation: ChannelMutationKind::Update,
        expected_version: created.version,
        fingerprint: "exact-test-intent".into(),
        desired: &created.definition,
    };
    let mut original = Reservation::acquire(
        &first.records,
        &fixture.keys,
        &created.definition,
        created.version,
        intent(),
    )
    .await
    .unwrap();
    let held = management::load(&other, &creator, &created.channel_id)
        .await
        .unwrap();
    let mut successor = Reservation::acquire(
        &other.records,
        &fixture.keys,
        &held.definition,
        held.record.version,
        intent(),
    )
    .await
    .unwrap();
    assert!(original
        .effect(&first.records, &fixture.keys, "bind:previous".into())
        .await
        .is_err());
    successor
        .effect(&other.records, &fixture.keys, "bind:current".into())
        .await
        .unwrap();
    assert!(original
        .retryable(&first.records, &fixture.keys)
        .await
        .is_err());
    assert_eq!(fixture.peer.calls.lock().unwrap().len(), 1);
    fixture.cleanup().await;
}
