use super::*;
use layer2_core::bridge::partition_commands::*;
use layer2_core::sandbox::store::AgentStore;
use mongodb::bson::Document;
use std::sync::atomic::{AtomicBool, AtomicUsize};
#[path = "partition_command_fixture.rs"]
mod support;
use support::*;
#[path = "partition_command_fault_tests.rs"]
mod faults;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_command_detach_replay_preserves_original_work_and_requires_explicit_reset() {
    let f = ReplyFixture::new().await;
    seed_original(&f).await;
    let adapter = adapter(&f);
    let service = BridgePartitionCommandService::new(f.svc.clone(), adapter.clone());
    let request = request(&f, &f.allocation, PartitionCommandAction::Detach).await;
    let context = f.context().await;
    let before = f
        .reply(&context, "before-partition-detach", "Original reply.")
        .await;
    assert_eq!(before["isError"], false);
    let receipt = service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "detach-once",
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.action, PartitionCommandAction::Detach);
    let detached = f
        .allocations
        .current(&f.allocation.identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detached.id, f.allocation.id);
    assert_eq!(detached.state, AllocationState::Retired);
    assert_eq!(detached.receipt, f.allocation.receipt);
    assert_eq!(f.context().await, context);
    assert_eq!(
        f.reply(&context, "after-partition-detach", "Still original.")
            .await["isError"],
        false
    );
    let proof = f
        .invokes
        .authorize(layer2_core::bridge::invoke::BridgeInvokeRequest {
            grant_id: f.grant.id.clone(),
            pin: f.inbox.pin.clone(),
            target: f.allocation.placement.target().clone(),
            effect: layer2_core::bridge::invoke::BridgeInvokeEffect::AdmitMessage {
                inbox_id: f.inbox.id.clone(),
            },
        })
        .await
        .unwrap();
    assert!(ChannelCommandReservation::prepare(proof, f.inbox.clone(), detached.clone()).is_err());
    assert!(matches!(
        f.allocations
            .reserve(
                AllocationReservation::new(
                    proposal(
                        &f.svc,
                        &f.bridges,
                        &f.allocation.pin.bridge_id,
                        "12345",
                        None
                    )
                    .await
                    .0,
                    proposal(
                        &f.svc,
                        &f.bridges,
                        &f.allocation.pin.bridge_id,
                        "12345",
                        None
                    )
                    .await
                    .1,
                    now()
                )
                .unwrap()
            )
            .await,
        Err(AllocationError::ResetRequired)
    ));
    assert_eq!(
        service
            .command(
                &caller(),
                None,
                &f.allocation.pin.bridge_id,
                "detach-once",
                request.clone()
            )
            .await
            .unwrap(),
        receipt
    );
    let reset = service
        .command(
            &caller(),
            None,
            &f.allocation.pin.bridge_id,
            "explicit-reset",
            support::request(&f, &detached, PartitionCommandAction::Reset).await,
        )
        .await
        .unwrap();
    assert_eq!(reset.successor.as_ref().unwrap().generation, 2);
    assert_eq!(adapter.preflights.load(Ordering::SeqCst), 1);
    // The original is now two revisions past Ready; both exact journals prove it.
    assert_eq!(f.context().await, context);
    assert_eq!(
        f.reply(&context, "after-detach-and-reset", "Same original target.")
            .await["isError"],
        false
    );
    let next = f
        .allocations
        .current(&f.allocation.identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.first_source, f.allocation.first_source);
    assert_eq!(next.config, f.allocation.config);
    assert!(f
        .resources
        .get_agent(&next.placement.target().agent_id)
        .await
        .unwrap()
        .is_none());
    adapter.fail_preflight.store(true, Ordering::SeqCst);
    let replay_reset = support::request(&f, &detached, PartitionCommandAction::Reset).await;
    assert_eq!(
        service
            .command(
                &caller(),
                None,
                &f.allocation.pin.bridge_id,
                "explicit-reset",
                replay_reset
            )
            .await
            .unwrap(),
        reset
    );
    let bridge = f
        .bridges
        .bridge(&f.allocation.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                "revision-after-detach",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .command(&caller(), None, &bridge.id, "detach-once", request)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(adapter.preflights.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_command_atomic_receipt_loss_and_concurrent_retry_create_one_successor() {
    let f = ReplyFixture::new().await;
    seed_original(&f).await;
    let adapter = adapter(&f);
    adapter.lose_response.store(true, Ordering::SeqCst);
    let service = BridgePartitionCommandService::new(f.svc.clone(), adapter.clone());
    let request = request(&f, &f.allocation, PartitionCommandAction::Reset).await;
    assert!(matches!(
        service
            .command(
                &caller(),
                None,
                &f.allocation.pin.bridge_id,
                "lost-reset",
                request.clone()
            )
            .await,
        Err(PartitionCommandError::Unavailable)
    ));
    adapter.fail_preflight.store(true, Ordering::SeqCst);
    let context = caller();
    let (a, b) = tokio::join!(
        service.command(
            &context,
            None,
            &f.allocation.pin.bridge_id,
            "lost-reset",
            request.clone()
        ),
        service.command(
            &context,
            None,
            &f.allocation.pin.bridge_id,
            "lost-reset",
            request.clone()
        )
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(adapter.preflights.load(Ordering::SeqCst), 1);
    let database = f.client.database(&f.database);
    assert_eq!(
        database
            .collection::<Document>("bridge_allocations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        database
            .collection::<Document>("bridge_partition_command_receipts")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let mut changed = request.clone();
    changed.action = PartitionCommandAction::Detach;
    assert!(matches!(
        service
            .command(
                &caller(),
                None,
                &f.allocation.pin.bridge_id,
                "lost-reset",
                changed
            )
            .await,
        Err(PartitionCommandError::Conflict)
    ));
    let mut outsider = caller();
    outsider.caller.subject = layer2_core::SubjectId::new("outsider");
    outsider.credential.subject = outsider.caller.subject.clone();
    outsider.actor = layer2_core::resource_access::ResourceActor::Human {
        subject: outsider.caller.subject.clone(),
    };
    assert!(service
        .command(
            &outsider,
            None,
            &f.allocation.pin.bridge_id,
            "lost-reset",
            request
        )
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_command_http_is_revision_checked_closed_and_recovers_202_receipt() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let f = ReplyFixture::new().await;
    seed_original(&f).await;
    let request = request(&f, &f.allocation, PartitionCommandAction::Detach).await;
    let service = BridgePartitionCommandService::new(f.svc.clone(), adapter(&f));
    let router = layer1_public::rest::bridge_partition_commands_router(Some(Arc::new(service)));
    let path = format!(
        "/api/v1/bridges/{}/partitions/{}/detach",
        f.allocation.pin.bridge_id.as_str(),
        f.allocation.id
    );
    let body = json!({"expected_bridge_revision":request.expected_bridge_revision,"expected_revision":request.expected_revision});
    let mut original = None;
    for (key, input, expected) in [
        (None, body.clone(), StatusCode::BAD_REQUEST),
        (
            Some("http-detach"),
            json!({"expected_bridge_revision":request.expected_bridge_revision,"expected_revision":request.expected_revision,"cleanup":true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (Some("http-detach"), body.clone(), StatusCode::ACCEPTED),
        (Some("http-detach"), body.clone(), StatusCode::ACCEPTED),
    ] {
        let mut builder = Request::builder()
            .method("POST")
            .uri(&path)
            .header("content-type", "application/json");
        if let Some(key) = key {
            builder = builder.header("idempotency-key", key);
        }
        let mut request = builder.body(Body::from(input.to_string())).unwrap();
        request.extensions_mut().insert(caller());
        request
            .extensions_mut()
            .insert(caller().caller.authorization);
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(caller().caller.subject));
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        if expected == StatusCode::ACCEPTED {
            if let Some(original) = &original {
                assert_eq!(&value, original);
            } else {
                original = Some(value.clone());
            }
            for secret in ["body_sha256", "custody", "grant_id", "token"] {
                assert!(!value.to_string().contains(secret), "{secret}");
            }
        }
    }
    f.close().await;
}
