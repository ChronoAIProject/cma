//! One unavailable renewal must fence its Agent without cancelling healthy work.
use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator};
use layer3_adapter::owner_protocol::TransportAuthority;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[path = "mongo_containment_tests/support.rs"]
mod support;
use super::super::mongo_renewal_faults::Fault;
use support::Fixture;

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn known_abort_exhaustion_contains_only_the_failed_agent() {
    isolated_renewal(Fault::KnownAbort).await;
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn uncertain_renewal_retains_custody_without_replay() {
    isolated_renewal(Fault::UnknownCommit).await;
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn committed_renewal_withheld_reply_retains_custody_without_replay() {
    isolated_renewal(Fault::WithheldReply).await;
}

async fn isolated_renewal(fault: Fault) {
    let fixture = Fixture::new().await;
    let original = fixture.lease(&fixture.faulted).await;
    let (send, mut receive) = tokio::sync::mpsc::channel(4);
    let streaming = tokio::spawn({
        let healthy = fixture.healthy.clone();
        async move {
            healthy
                .run(async {
                    while let Some(value) = receive.recv().await {
                        let (sequence, reply): (u32, tokio::sync::oneshot::Sender<u32>) = value;
                        reply
                            .send(sequence)
                            .expect("stream consumer remains present");
                    }
                })
                .await
        }
    });
    let (reply, frame) = tokio::sync::oneshot::channel();
    send.send((1, reply)).await.unwrap();
    assert_eq!(frame.await.unwrap(), 1);

    fixture.arm(fault).await;
    let started = tokio::time::Instant::now();
    let result = fixture.manager.maintain().await;
    let elapsed = started.elapsed();
    fixture.disarm().await;
    let actual = fixture.lease(&fixture.faulted).await;
    let returned = fixture.router.returned.lock().unwrap().clone();
    eprintln!(
        "renewal_containment fault={fault:?} elapsed_ms={} store_returned={} manager_ready={} healthy_current={} failed_current={}",
        elapsed.as_millis(), returned.is_some(), fixture.manager.is_ready(),
        fixture.healthy.is_current(), fixture.faulted.is_current()
    );
    assert_eq!(actual.token, original.token);
    match fault {
        Fault::KnownAbort => {
            assert_eq!(actual, original, "aborted renewal changed the lease");
            assert!(elapsed >= Duration::from_millis(1_900));
            assert!(elapsed < Duration::from_secs(3), "renewal budget widened");
        }
        Fault::UnknownCommit => {
            assert!(matches!(returned, Some(OwnershipError::Unavailable(_))));
            // An unknown commit can leave either the old or renewed lease.
            // Both must retain exact custody without replaying the renewal.
            assert!(actual.expires_at_ms >= original.expires_at_ms);
        }
        Fault::WithheldReply => {
            assert!(
                returned.is_none(),
                "withheld store reply reached coordinator"
            );
            assert!(actual.expires_at_ms > original.expires_at_ms);
            assert!(elapsed >= Duration::from_millis(1_900));
            assert!(elapsed < Duration::from_secs(3), "renewal budget widened");
        }
    }
    assert!(!fixture.faulted.is_current());
    assert!(fixture
        .faulted
        .run(async { panic!("cancelled effect executed") })
        .await
        .is_err());
    // Capture the RED before cleanup; a failed assertion must not strand the
    // disposable database or the bounded stream task.
    let manager_ready = fixture.manager.is_ready();
    let healthy_current = fixture.healthy.is_current();
    let failure_signal = *fixture.manager.failure_signal().borrow();
    if result.is_err() || !manager_ready || !healthy_current || failure_signal {
        drop(send);
        streaming
            .await
            .unwrap()
            .expect_err("global failure cancelled the stream");
        fixture.close().await;
        panic!("isolated renewal disabled healthy authority: result={:?}, ready={manager_ready}, healthy={healthy_current}, failed={failure_signal}", result.as_ref().map(Vec::len));
    }
    assert!(fixture.faulted.receiver_cancelled());
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 1);
    assert!(fixture
        .manager
        .acquire(&fixture.faulted.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .manager
        .cancelled_receivers()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &fixture.faulted)));
    fixture.healthy_effect("after-renewal-failure").await;
    let (reply, frame) = tokio::sync::oneshot::channel();
    send.send((2, reply)).await.unwrap();
    assert_eq!(frame.await.unwrap(), 2);
    fixture.manager.maintain().await.unwrap();
    assert_eq!(
        fixture.router.calls.load(Ordering::SeqCst),
        1,
        "cancelled renewal replayed"
    );
    let retained = fixture.lease(&fixture.faulted).await;
    assert_eq!(retained.token, actual.token);
    if matches!(fault, Fault::UnknownCommit) {
        // The original uncertain transaction may become visible later. The
        // call counter, not an unchanged expiry, proves no renewal replay.
        assert!(retained.expires_at_ms >= actual.expires_at_ms);
    } else {
        assert_eq!(retained, actual);
    }
    assert_eq!(fixture.router.releases.load(Ordering::SeqCst), 0);
    assert!(
        TransportAuthority::renew_for_challenge(fixture.faulted.as_ref())
            .await
            .is_err()
    );
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 1);
    fixture.verify_cleanup(&retained, fault).await;
    drop(send);
    streaming.await.unwrap().unwrap();
    fixture.close().await;
}

#[cfg(feature = "qualification-hooks")]
#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn qualification_routes_the_exact_renewal_to_the_isolated_mongo_client() {
    let fixture = Fixture::new().await;
    let original = fixture.lease(&fixture.faulted).await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("use disposable Mongo");
    let isolated =
        MongoAgentOwnership::prepare_qualification_renewal(&uri, fixture.store.database.name())
            .await
            .unwrap();
    let capture = fixture
        .manager
        .capture_qualification_owner(&fixture.faulted.token)
        .await
        .unwrap();
    let registry = cma_qualification::Registry::default();
    assert!(fixture
        .manager
        .attach_qualification_renewal(Arc::new(registry.clone())));
    let arm = registry
        .arm_renewal(capture, isolated, Duration::from_secs(6))
        .unwrap();
    let started = tokio::time::Instant::now();
    fixture.manager.maintain().await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(1900));
    assert_eq!(
        arm.evidence(),
        cma_qualification::RenewalEvidence {
            started: true,
            result: Some(cma_qualification::RenewalOutcome::Committed),
            caller_dropped: true,
        }
    );
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 0);
    let durable = fixture.lease(&fixture.faulted).await;
    assert_eq!(durable.token, original.token);
    assert!(durable.expires_at_ms > original.expires_at_ms);
    assert!(fixture.faulted.receiver_cancelled());
    assert!(fixture.healthy.is_current());
    fixture
        .healthy_effect("qualification-renewal-healthy")
        .await;
    fixture.verify_cleanup(&durable, Fault::WithheldReply).await;
    fixture.close().await;
}

#[cfg(feature = "qualification-hooks")]
#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn qualification_retirement_retains_the_selected_renewal_until_its_store_result() {
    use super::super::mongo_renewal_faults::RoutedStore;
    use layer2_core::agent_ownership::MaintenancePolicy;

    let fixture = Fixture::with_policy(MaintenancePolicy::ContainFailures).await;
    let selected = RoutedStore::new(
        fixture.store.clone(),
        fixture.faulted.token.agent_id.clone(),
    )
    .await;
    selected.arm(Fault::WithheldReply).await;
    let capture = fixture
        .manager
        .capture_qualification_owner(&fixture.faulted.token)
        .await
        .unwrap();
    let registry = cma_qualification::Registry::default();
    assert!(fixture
        .manager
        .attach_qualification_renewal(Arc::new(registry.clone())));
    let mut armed = registry
        .arm_renewal(capture, selected.clone(), Duration::from_secs(8))
        .unwrap();

    let maintenance = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.maintain().await }
    });
    tokio::time::timeout(Duration::from_secs(3), selected.wait_for_held_reply())
        .await
        .expect("the selected Mongo renewal issued and its result is held");
    assert!(!maintenance.is_finished());
    assert!(tokio::time::timeout(
        Duration::from_millis(150),
        TransportAuthority::renew_for_challenge(fixture.faulted.as_ref()),
    )
    .await
    .is_err());
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 0);
    assert_eq!(selected.calls.load(Ordering::SeqCst), 1);

    maintenance.await.unwrap().unwrap();
    assert!(armed.evidence().caller_dropped);
    assert!(armed.evidence().result.is_none());
    assert!(fixture.faulted.receiver_cancelled());
    fixture.acknowledge_receiver();
    let retained = fixture.lease(&fixture.faulted).await;
    assert!(fixture
        .manager
        .handoff(&fixture.faulted, fixture.faulted.revoke_transports())
        .await
        .is_err());
    assert_eq!(fixture.lease(&fixture.faulted).await, retained);
    assert_eq!(fixture.router.releases.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 0);
    assert_eq!(selected.calls.load(Ordering::SeqCst), 1);
    fixture
        .healthy_effect("while-selected-renewal-unresolved")
        .await;

    selected.allow_held_reply();
    tokio::time::timeout(Duration::from_secs(2), async {
        while armed.evidence().result.is_none() {
            armed.changed().await.expect("selected renewal evidence");
        }
    })
    .await
    .unwrap();
    assert_eq!(
        armed.evidence().result,
        Some(cma_qualification::RenewalOutcome::Committed)
    );
    fixture
        .manager
        .handoff(&fixture.faulted, fixture.faulted.revoke_transports())
        .await
        .unwrap();
    assert_eq!(fixture.router.releases.load(Ordering::SeqCst), 1);
    assert!(fixture
        .store
        .owner(&fixture.faulted.token.agent_id)
        .await
        .unwrap()
        .is_none());
    fixture.close().await;
}
