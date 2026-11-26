//! Coordinator cancellation and late-commit regressions; deterministic store only.
use super::*;
use layer3_adapter::owner_protocol::{
    OwnershipFailure, OwnershipFailureReason as Reason, OwnershipPhase as Phase, TransportAuthority,
};
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[path = "receiver_loss_test_support.rs"]
mod support;
use support::Fixture;

#[tokio::test(start_paused = true)]
async fn receiver_check_cancellation_preserves_healthy_owners_and_unacknowledged_lease() {
    let fixture = Fixture::new(false).await;
    let original = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    // Exercise the production adapter-authority entry: one failed store check
    // irrevocably cancels this authority, then the store becomes accessible.
    fixture.store.inner.partition(true);
    assert!(TransportAuthority::check(fixture.cancelled.as_ref())
        .await
        .is_err());
    fixture.store.inner.partition(false);
    assert!(!fixture.cancelled.is_current());
    fixture.store.check(&fixture.cancelled.token).await.unwrap();
    let cleanup = fixture.manager.maintain().await;
    assert!(
        cleanup.is_ok(),
        "a cancelled receiver must not fail healthy local owners: {:?}",
        cleanup.as_ref().err()
    );
    assert!(fixture
        .manager
        .cancelled_receivers()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &fixture.cancelled)));
    assert_eq!(
        fixture
            .store
            .cancelled_owner_renewals
            .load(Ordering::SeqCst),
        0,
        "pre-cancelled authority must not extend its durable expiry"
    );
    fixture.healthy_work().await;
    fixture
        .refuse_then_acknowledge(original.expires_at_ms)
        .await;
    assert_eq!(
        fixture
            .store
            .cancelled_owner_renewals
            .load(Ordering::SeqCst),
        0
    );
    // Containment must not turn real membership/store loss into availability.
    // A transient partition only skips ticks; it extends nothing, so the
    // established membership and healthy owner still end on schedule.
    fixture.store.inner.partition(true);
    let mut ticks = 0;
    let failure = loop {
        tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
        ticks += 1;
        fixture.peer.maintain().await.unwrap();
        match fixture.manager.maintain().await {
            Ok(handoffs) => assert!(handoffs.is_empty()),
            Err(error) => break error,
        }
        assert!(ticks <= DEFAULT_LEASE_MS / HEARTBEAT_INTERVAL_MS);
    };
    assert_eq!(failure, OwnershipError::Stale);
    assert!(!fixture.manager.is_ready());
    assert!(!fixture.healthy.is_current());
    fixture.peer.maintain().await.unwrap();
    assert!(fixture.peer_owner.is_current());
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_real_renewal_never_revives_or_releases_before_receiver_ack() {
    for renew_before_pause in [false, true] {
        let fixture = Fixture::new(renew_before_pause).await;
        let original = fixture
            .store
            .owner(&fixture.cancelled.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        tokio::time::advance(Duration::from_secs(3)).await;
        fixture.store.pause_next.store(true, Ordering::SeqCst);
        let maintaining = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain().await }
        });
        fixture.store.entered.notified().await;
        TransportAuthority::revoke(fixture.cancelled.as_ref());
        tokio::time::advance(Duration::from_secs(1)).await;
        fixture.store.resume.notify_one();
        let cleanup = maintaining.await.unwrap();
        assert!(
            cleanup.is_ok(),
            "in-flight renewal of cancelled receiver must not fail healthy owners: {:?}",
            cleanup.as_ref().err()
        );
        assert!(fixture
            .manager
            .cancelled_receivers()
            .await
            .iter()
            .any(|owner| Arc::ptr_eq(owner, &fixture.cancelled)));
        assert!(!fixture.cancelled.is_current());
        let actual = fixture
            .store
            .owner(&fixture.cancelled.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.token, original.token);
        assert!(
            actual.expires_at_ms > original.expires_at_ms,
            "already-started durable renewal may finish despite local cancellation"
        );
        fixture.healthy_work().await;
        fixture.refuse_then_acknowledge(actual.expires_at_ms).await;
        assert_eq!(
            fixture
                .store
                .cancelled_owner_renewals
                .load(Ordering::SeqCst),
            1,
            "quarantine must prevent subsequent durable renewals"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn expired_observation_before_late_commit_never_reacquires_the_cancelled_token() {
    expiry_before_late_commit(MaintenancePolicy::Legacy).await;
}

#[tokio::test(start_paused = true)]
async fn contained_expiry_observation_preserves_floor_through_late_same_token_commit() {
    expiry_before_late_commit(MaintenancePolicy::ContainFailures).await;
}

async fn expiry_before_late_commit(policy: MaintenancePolicy) {
    let fixture = Fixture::with_policy(true, policy).await;
    let original = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    // Begin within the old lease. The renewal sees a valid token and computes a
    // later expiry, but its publication is held behind a deterministic barrier.
    tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS - 1_000)).await;
    fixture.peer.maintain().await.unwrap();
    fixture.store.pause_next.store(true, Ordering::SeqCst);
    fixture
        .store
        .hide_renewal_until_publish
        .store(true, Ordering::SeqCst);
    let maintaining = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.maintain().await }
    });
    fixture.store.entered.notified().await;
    TransportAuthority::revoke(fixture.cancelled.as_ref());
    tokio::time::advance(Duration::from_millis(1_500)).await;
    assert!(fixture.store.membership().await.unwrap().now_ms > original.expires_at_ms);
    assert!(fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .is_none());
    // Absence at the old expiry cannot prove that the in-flight renewal aborted.
    // Removing this custody is the unsafe draft behavior the regression targets.
    assert!(
        !fixture
            .manager
            .forget_cancelled(&fixture.cancelled)
            .await
            .unwrap(),
        "expiry observation must retain a cancelled-token floor across an unknown renewal outcome"
    );
    if policy == MaintenancePolicy::ContainFailures {
        // Exercise both the initial local join and the next attempt's actual
        // absence observation while the same-token renewal is still withheld.
        for _ in 0..2 {
            assert_eq!(
                fixture
                    .manager
                    .retirement_background(&fixture.cancelled)
                    .await
                    .unwrap(),
                None
            );
        }
    }
    fixture.store.resume.notify_one();
    assert!(maintaining.await.unwrap().is_ok());
    let committed = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(committed.token, original.token);
    assert!(committed.expires_at_ms > fixture.store.membership().await.unwrap().now_ms);
    assert!(fixture.manager.acquire(&original.token.agent_id).await.unwrap().is_none(),
        "a late same-token renewal must never create a new current Authority for a cancelled receiver");
    assert!(!fixture.cancelled.is_current());
    assert!(fixture
        .manager
        .local(&original.token.agent_id)
        .await
        .is_none());
    assert!(fixture
        .manager
        .owned()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &fixture.cancelled)));
    fixture.healthy_work().await;
    fixture
        .refuse_then_acknowledge(committed.expires_at_ms)
        .await;
    let mut successor = None;
    for manager in [&fixture.manager, &fixture.peer] {
        if let Some(authority) = manager.acquire(&original.token.agent_id).await.unwrap() {
            assert!(successor.is_none());
            assert!(authority.token.epoch > original.token.epoch);
            successor = Some(authority);
        }
    }
    let successor = successor.expect("acknowledged release permits one strictly newer owner");
    // A delayed old cleanup cannot shorten or delete the successor's lease.
    fixture
        .manager
        .handoff(&fixture.cancelled, fixture.cancelled.revoke_transports())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .owner(&original.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        successor.token
    );
    assert!(successor.is_current());
}

#[tokio::test(start_paused = true)]
async fn delayed_receiver_cancellation_cannot_relabel_generic_revocation_or_expiry() {
    for expires in [false, true] {
        let fixture = Fixture::new(false).await;
        if expires {
            tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS)).await;
        } else {
            fixture.cancelled.revoke();
        }
        assert!(!fixture.cancelled.is_current());
        TransportAuthority::revoke(fixture.cancelled.as_ref());
        assert!(
            !fixture.cancelled.receiver_cancelled(),
            "a late receiver callback cannot make generic authority loss recoverable"
        );
        assert!(fixture.manager.maintain().await.is_err());
        assert!(!fixture.manager.is_ready());
        assert!(!fixture.healthy.is_current());
    }
}

#[tokio::test]
async fn http_failure_survives_cancelled_worker_and_failed_cleanup() {
    use layer3_adapter::hosted::{HostedControlTransport, ProviderHostedControlTransport};
    use layer3_adapter::{provider::SandboxClient, wire::RequestFrame, RuntimeHandle};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fixture = Fixture::new(false).await;
    let original = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{address}"),
        "fixture-key",
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        for action in [
            "endpoints/9090",
            "manager-ownership/challenge",
            "manager-ownership/revoke",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).await.unwrap(), 1);
                bytes.push(byte[0]);
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8(bytes).unwrap();
            assert!(headers.lines().next().unwrap().contains(action));
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            let (status, body) = if action.starts_with("endpoints") {
                (
                    "200 OK",
                    serde_json::json!({"endpoint":format!("http://{address}")}).to_string(),
                )
            } else if action.ends_with("challenge") {
                ("404 Not Found", "private-response-marker".into())
            } else {
                ("403 Forbidden", "private-response-marker".into())
            };
            stream.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    });
    let result = fixture
        .cancelled
        .run(async {
            let _result = transport
                .exchange(
                    &RuntimeHandle::new("fixture-runtime"),
                    RequestFrame::Resources,
                )
                .await;
            // The normal worker-return path can be discarded after receiver cancellation.
            std::future::pending::<()>().await
        })
        .await;
    assert_eq!(result, Err(OwnershipError::Stale));
    let expected = OwnershipFailure::http(Phase::Challenge, 404);
    assert_eq!(fixture.cancelled.receiver_failure(), Some(expected));
    assert_eq!(
        fixture
            .store
            .inner
            .recorded_failure(&fixture.cancelled.token.agent_id),
        Some((fixture.cancelled.token.clone(), expected))
    );
    assert!(fixture.cancelled.receiver_cancelled());
    assert_eq!(
        fixture
            .store
            .owner(&fixture.cancelled.token.agent_id)
            .await
            .unwrap(),
        original
    );
    assert!(fixture.cancelled.revoke_transports().await.is_err());
    server.await.unwrap();
    assert_eq!(fixture.cancelled.receiver_failure(), Some(expected));
    assert_eq!(
        fixture
            .store
            .inner
            .recorded_failure(&fixture.cancelled.token.agent_id),
        Some((fixture.cancelled.token.clone(), expected))
    );
    fixture.healthy_work().await;
}

#[tokio::test(start_paused = true)]
async fn store_failure_is_recorded_before_cancelled_scope_loses_its_result() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        let fixture = Fixture::new(false).await;
        fixture.store.inner.partition(true);
        let result = fixture
            .cancelled
            .run(async {
                let _ = fixture.cancelled.check_for_phase(phase).await;
                std::future::pending::<()>().await
            })
            .await;
        assert_eq!(result, Err(OwnershipError::Stale));
        assert_eq!(
            fixture.cancelled.receiver_failure(),
            Some(OwnershipFailure::new(phase, Reason::TransportUnavailable))
        );
        fixture.store.inner.partition(false);
        fixture.healthy_work().await;
    }
}

#[tokio::test(start_paused = true)]
async fn renewal_deadline_is_retained_before_receiver_cancellation() {
    let fixture = Fixture::new(false).await;
    fixture.store.pause_next.store(true, Ordering::SeqCst);
    let pending = tokio::spawn({
        let authority = fixture.cancelled.clone();
        async move {
            authority
                .run(async {
                    let _ = authority.renew_for_challenge().await;
                    std::future::pending::<()>().await
                })
                .await
        }
    });
    fixture.store.entered.notified().await;
    tokio::time::advance(Duration::from_millis(STORE_DEADLINE_MS)).await;
    assert_eq!(pending.await.unwrap(), Err(OwnershipError::Stale));
    assert_eq!(
        fixture.cancelled.receiver_failure(),
        Some(OwnershipFailure::new(Phase::Renew, Reason::Deadline))
    );
    assert!(fixture.cancelled.receiver_cancelled());
    fixture.healthy_work().await;
}

#[path = "receiver_failure_tests.rs"]
mod persistence;

#[path = "receiver_diagnostic_tests.rs"]
mod diagnostics;
