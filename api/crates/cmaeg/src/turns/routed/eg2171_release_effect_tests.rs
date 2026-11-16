//! Release of an uncertain continuation restores its unresolved parent frontier.
use super::{release_test_fixture::ReleaseHarness, *};
use crate::{
    delivery::{
        durable::{receipt_for, DeliveryCoordinator},
        intents::{DeliveryCommand, DeliveryOutcome},
        router::ShardedScheduler,
    },
    replicas::partition::PartitionLease,
};
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};
use tokio::sync::mpsc;

struct Promoted {
    id: String,
    captured: PartitionLease,
    owner: usize,
    saved: Value,
    version: u64,
    rx: mpsc::Receiver<DeliveryCommand>,
}

async fn promote(h: &ReleaseHarness, unknown: bool) -> Promoted {
    let plan = &h.plans[0];
    let target = &plan.targets[0];
    let (tx, rx) = mpsc::channel(4);
    let tools = RoutedToolReplies::new(
        h.nodes[0].plans(),
        h.nodes[0].journal(),
        h.current.clone(),
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
    );
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let capability = cmaeg_core::seal::ReplyTargetPayload {
        v: 1,
        provider: target.launch.provider.slug.clone(),
        binding_id: target.launch.provider.binding_id.clone(),
        agent_key_hash: [9; 32],
        issued_at: now,
        expires_at: target.launch.event.expires_at,
        event: model::delivery_event(plan, target),
    };
    assert!(matches!(
        tools
            .reply_delayed(
                replica_test_fixture::caller(),
                &capability,
                "uncertain-child",
                replica_test_fixture::message()
            )
            .await,
        Err(ExecutionError::Busy)
    ));
    let roots = h.nodes[0].journal().effect_roots(None, 32).await.unwrap();
    assert_eq!(roots.len(), 1);
    let (id, mut saved, version) = roots.into_iter().next().unwrap();
    let until = tokio::time::Instant::now() + Duration::from_secs(25);
    let (owner, captured) = 'claim: loop {
        for (owner, node) in h.nodes.iter().enumerate() {
            match node.claim_effect(plan, target, &id).await {
                Ok(token) => break 'claim (owner, token),
                Err(crate::replicas::Error::Busy | crate::replicas::Error::Lost) => {}
                Err(error) => panic!("continuation claim failed: {error:?}"),
            }
        }
        assert!(
            tokio::time::Instant::now() < until,
            "continuation never became the head"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(h.queue().await, vec![id.clone(), target.id.clone()]);
    let mut receipt = receipt_for(DeliveryOutcome::Unknown {
        reason: "fixture-confirmation-lost".into(),
    });
    let reference = cmaeg_core::ids::MessageRef::seal(
        &serde_json::from_value(json!({
            "channel_id": capability.event.channel_id, "event_id": capability.event.event_id,
            "consumer": capability.event.consumer, "platform_message_id": "fixture-confirmed-part",
            "route_id": "telegram:77:42"
        }))
        .unwrap(),
        &h.database.keys(),
    )
    .unwrap();
    receipt
        .message_parts
        .push(crate::delivery::intents::ConfirmedMessagePart {
            index: 0,
            message_ref: reference,
        });
    saved["state"] = if unknown {
        json!({"phase":"unknown", "receipt":receipt})
    } else {
        json!({"phase":"sending"})
    };
    // Model the pre-crash journal checkpoint through the same captured resource
    // fence as production. This fixture never sends a provider/native request.
    let version = h.nodes[owner]
        .journal()
        .write_effect_fenced(
            &Some(captured.clone()),
            &id,
            &plan.identity.owner,
            plan.retained_until,
            &saved,
            Some(version),
            true,
        )
        .await
        .unwrap();
    h.nodes[owner].partitions.release(&captured).await.unwrap();
    Promoted {
        id,
        captured,
        owner,
        saved,
        version,
        rx,
    }
}

async fn assert_preserved(
    h: &ReleaseHarness,
    child: &mut Promoted,
    request: &unresolved::ReleaseRequest,
    result: &unresolved::ReleaseResult,
) {
    assert_eq!(result.released_head, child.id);
    assert_eq!(h.queue().await, vec![h.plans[0].targets[0].id.clone()]);
    assert_eq!(
        h.state(0).await.phase,
        Phase::Unknown,
        "child release must not release the parent"
    );
    let (saved, _) = h.record(&child.id, true).await;
    assert_eq!(
        saved["state"]["receipt"], child.saved["state"]["receipt"],
        "uncertain delivery and any confirmed parts must survive release"
    );
    assert_eq!(saved["state"]["phase"], "unknown_abandoned");
    assert_eq!(saved["state"]["operation_id"], request.operation_id);
    let audit_id = crate::connections::store::index_id(
        "cmaeg-unresolved-release-v1",
        &[&h.partition, &child.id],
    );
    let audit = h.record(&audit_id, false).await;
    assert_eq!(audit.0["original_phase"], child.saved["state"]);
    assert_eq!(audit.0["result"], serde_json::to_value(result).unwrap());
    let item = crate::connections::store::index_id("cmaeg-partition-item-v1", &[&child.id]);
    assert_eq!(h.record(&item, true).await.0["done"], true);
    let head_after = h.record(&h.partition, false).await;
    assert_eq!(h.release(1, request).await.unwrap(), *result);
    assert_eq!(h.record(&h.partition, false).await, head_after);
    assert_eq!(h.record(&child.id, true).await.0, saved);
    assert_eq!(h.record(&audit_id, false).await, audit);
    assert!(
        h.nodes[0]
            .journal()
            .effect_roots(None, 32)
            .await
            .unwrap()
            .is_empty(),
        "released child must not remain active for automatic replay"
    );
    let stale = h.nodes[child.owner]
        .journal()
        .write_effect_fenced(
            &Some(child.captured.clone()),
            &child.id,
            &h.plans[0].identity.owner,
            h.plans[0].retained_until,
            &child.saved,
            Some(child.version),
            true,
        )
        .await;
    assert_eq!(
        stale,
        Err(ExecutionError::LostLease),
        "former child owner cannot publish after release"
    );
    let parent_request = h.request().await;
    assert_eq!(parent_request.expected_head, h.plans[0].targets[0].id);
    assert_eq!(h.state(0).await.phase, Phase::Unknown);
    assert!(
        child.rx.try_recv().is_err(),
        "release must not send or replay output"
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn eg2171_promoted_sending_and_unknown_child_release_preserves_parent_and_receipt() {
    for unknown in [false, true] {
        let h = ReleaseHarness::with_successors(false).await;
        let result = AssertUnwindSafe(async {
            let mut child = promote(&h, unknown).await;
            let request = h.request().await;
            assert_eq!(request.expected_head, child.id);
            assert!(matches!(
                h.release(0, &request).await,
                Err(ExecutionError::Expired)
            ));
            h.expired().await;
            let request = h.request().await;
            let result = h.release(0, &request).await.unwrap();
            assert_preserved(&h, &mut child, &request, &result).await;
        })
        .catch_unwind()
        .await;
        h.cleanup().await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn eg2171_two_replicas_race_uncertain_child_release_without_consuming_parent() {
    let h = ReleaseHarness::with_successors(false).await;
    let result = AssertUnwindSafe(async {
        let mut child = promote(&h, true).await;
        h.expired().await;
        let first = h.request().await;
        let mut second = first.clone();
        second.operation_id = uuid::Uuid::new_v4().to_string();
        let (a, b) = tokio::join!(h.release(0, &first), h.release(1, &second));
        assert_eq!(
            usize::from(a.is_ok()) + usize::from(b.is_ok()),
            1,
            "one exact child head has one winning release: {a:?} / {b:?}"
        );
        let (winner, loser, result) = match (a, b) {
            (Ok(result), Err(ExecutionError::Busy | ExecutionError::LostLease)) => {
                (&first, &second, result)
            }
            (Err(ExecutionError::Busy | ExecutionError::LostLease), Ok(result)) => {
                (&second, &first, result)
            }
            other => panic!("release race did not fence loser: {other:?}"),
        };
        assert!(matches!(
            h.release(0, loser).await,
            Err(ExecutionError::LostLease)
        ));
        assert_preserved(&h, &mut child, winner, &result).await;
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
