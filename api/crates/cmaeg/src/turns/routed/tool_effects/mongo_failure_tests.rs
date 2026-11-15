use super::{super::replica_test_fixture::*, mongo_tests::execute, *};
use crate::{
    delivery::intents::DeliveryOutcome,
    tools::reply_targets::receipts::{Admission as LegacyAdmission, Store as LegacyStore},
    turns::routed::delivery_tests::command,
};
use std::sync::atomic::Ordering;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn cancelled_send_cannot_finish_after_successor_retains_unknown() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    let payload = Payload {
        capability: h.capability(),
        caller: caller().key_hash,
        message: message(),
    };
    let store = Store(h.nodes[0].journal());
    let root = store
        .admit(&h.plan, h.target(), "cancel", &payload)
        .await
        .unwrap();
    h.nodes[0]
        .journal()
        .enqueue_effect(&h.plan, h.target(), &root.id)
        .await
        .unwrap();
    let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
    let reply = {
        let execution = execute(&h, &root.id);
        tokio::pin!(execution);
        let reply = tokio::select! {
            _ = &mut execution => panic!("send must be awaiting its receipt"),
            reply = command(&mut rx) => reply,
        };
        let _permit = reply.gate.attempt().await.unwrap();
        assert!(reply.gate.commit());
        reply
        // Dropping the execution future models process cancellation after the
        // native request may have reached its destination, before its receipt.
    };
    assert!(!reply.gate.commit());
    drop(h.nodes[owner].worker.take());
    execute(&h, &root.id).await;
    assert!(reply.gate.attempt().await.is_err());
    assert!(reply.receipt.send(receipt()).is_err());
    let saved = store
        .read(&root.id, &h.plan, h.target())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        saved.state,
        model::State::Unknown { receipt: None }
    ));
    let result = h.tools[1 - owner]
        .reply_delayed(caller(), &payload.capability, "cancel", message())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
    assert_eq!(
        h.nodes[1 - owner]
            .journal()
            .effect_roots(None, 32)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(rx.try_recv().is_err());
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_capability_rechecks_authority_before_admission_and_execution() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    let mut capability = h.capability();
    capability.issued_at -= 60;
    capability.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() - 1;
    assert!(h.tools[0]
        .reply_delayed(caller(), &capability, "expired", message())
        .await
        .is_err());
    let capability = h.capability();
    let payload = Payload {
        capability: capability.clone(),
        caller: caller().key_hash,
        message: message(),
    };
    let store = Store(h.nodes[0].journal());
    let root = store
        .admit(&h.plan, h.target(), "queued", &payload)
        .await
        .unwrap();
    h.nodes[0]
        .journal()
        .enqueue_effect(&h.plan, h.target(), &root.id)
        .await
        .unwrap();
    h.current.revoke_at.store(0, Ordering::SeqCst);
    assert!(matches!(
        h.tools[0]
            .reply_delayed(caller(), &capability, "revoked", message())
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    execute(&h, &root.id).await;
    let saved = store
        .read(&root.id, &h.plan, h.target())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        store.receipt(&saved).await.unwrap().unwrap().delivery,
        DeliveryOutcome::Rejected { .. }
    ));
    assert!(h.nodes[0]
        .journal()
        .effect_roots(None, 32)
        .await
        .unwrap()
        .is_empty());
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn legacy_receipts_and_unresolved_fences_prevent_new_effect_admission() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    let capability = h.capability();
    let journal = h.nodes[0].journal();
    let legacy = LegacyStore::new(journal.records.clone(), h.database.keys());
    let LegacyAdmission::Claimed(claim) = legacy
        .claim(&capability, "confirmed", &message(), &h.plan.identity.owner)
        .await
        .unwrap()
    else {
        panic!("legacy claim")
    };
    legacy.finish(claim, &receipt()).await.unwrap();
    let replay = h.tools[1]
        .reply_delayed(caller(), &capability, "confirmed", message())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(replay.delivery, DeliveryOutcome::Confirmed { .. }));
    let mut changed = message();
    changed.text = Some("changed legacy content".into());
    assert!(matches!(
        h.tools[1]
            .reply_delayed(caller(), &capability, "confirmed", changed)
            .await,
        Err(ExecutionError::IdempotencyConflict)
    ));
    let LegacyAdmission::Claimed(_abandoned) = legacy
        .claim(&capability, "abandoned", &message(), &h.plan.identity.owner)
        .await
        .unwrap()
    else {
        panic!("legacy pending claim")
    };
    for key in ["abandoned", "different-key"] {
        let replay = h.tools[1]
            .reply_delayed(caller(), &capability, key, message())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    }
    assert!(h.nodes[0]
        .journal()
        .effect_roots(None, 32)
        .await
        .unwrap()
        .is_empty());
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_capability_cannot_reopen_a_refused_parent() {
    let mut h = Harness::new().await;
    let (owner, lease) = h.accepted().await;
    let journal = h.nodes[owner].journal();
    let (mut parent, version) = journal
        .read::<TargetState>(&h.target().id)
        .await
        .unwrap()
        .unwrap();
    parent.phase = Phase::Refused {
        code: "fixture-refused".into(),
    };
    journal
        .write_fenced(
            &lease.partition,
            &h.target().id,
            &h.plan.identity.owner,
            h.plan.retained_until,
            &parent,
            Some(version),
        )
        .await
        .unwrap();
    assert!(matches!(
        h.tools[owner]
            .reply_delayed(caller(), &h.capability(), "refused", message())
            .await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(journal.effect_roots(None, 32).await.unwrap().is_empty());
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}
