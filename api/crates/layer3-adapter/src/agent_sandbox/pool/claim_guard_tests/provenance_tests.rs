//! Exercise provenance at the HTTP boundary, including a previously valid Pending read.
use super::super::{
    fixture::{prefix, ResponseGate},
    home_tests::VOLUME,
};
use super::{authority_tests, fixture, Fixture, NativePoolClaim, MEMBER};
use crate::{agent_sandbox::NativeClaimGuardReason, AdapterError};
use std::{sync::atomic::Ordering, time::Duration};

async fn entered(gate: &ResponseGate) {
    tokio::time::timeout(Duration::from_secs(3), gate.entered.notified())
        .await
        .expect("observation did not reach the selected HTTP response");
}

fn path(resource: &str, claim: &NativePoolClaim) -> String {
    match resource {
        "claim" => format!("{}/sandboxclaims/{}", prefix(), claim.name),
        "member" => MEMBER.into(),
        "pvc" => VOLUME.into(),
        _ => unreachable!(),
    }
}

async fn pending_fixture(enabled: bool) -> (Fixture, NativePoolClaim) {
    let (mut fixture, claim) = fixture(true).await;
    fixture.client.config.native_claim_guard_enabled = enabled;
    fixture.state.lock().unwrap().native_annotation_generation = true;
    fixture.state.lock().unwrap().calls.clear();
    (fixture, claim)
}

async fn later_stall(resource: &str) {
    for enabled in [false, true] {
        let (fixture, claim) = pending_fixture(enabled).await;
        let call = format!("GET {}", path(resource, &claim));
        let first = fixture.gate(call.clone(), Some(format!("PATCH {MEMBER}")));
        let stalled = fixture.gate(call, None);
        let started = tokio::time::Instant::now();
        let (result, unchanged) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(8), fixture.client.adopt_home(&claim)),
            async {
                entered(&first).await;
                first.release.notify_one();
                entered(&stalled).await;
                fixture.state.lock().unwrap().rows.clone()
            }
        );
        assert_eq!(
            result.unwrap().unwrap_err(),
            AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut)
        );
        assert!(started.elapsed() >= Duration::from_secs(if enabled { 2 } else { 5 }));
        assert_eq!(fixture.state.lock().unwrap().rows, unchanged);
        let authority = authority_tests::published(&fixture, &claim);
        stalled.release.notify_one();
        authority_tests::retry(&fixture, &claim, &authority).await;
    }
}

#[tokio::test]
async fn pending_then_stalled_claim_read_keeps_observation_timeout() {
    later_stall("claim").await;
}

#[tokio::test]
async fn pending_then_stalled_member_read_keeps_observation_timeout() {
    later_stall("member").await;
}

#[tokio::test]
async fn pending_then_stalled_pvc_read_keeps_observation_timeout() {
    later_stall("pvc").await;
}

async fn transport_failure(resource: &str) {
    for enabled in [false, true] {
        for prior_pending in [false, true] {
            let (fixture, claim) = pending_fixture(enabled).await;
            let call = format!("GET {}", path(resource, &claim));
            let first = fixture.gate(call.clone(), Some(format!("PATCH {MEMBER}")));
            let failure = if prior_pending {
                fixture.gate(call, None)
            } else {
                first.clone()
            };
            failure.disconnect.store(true, Ordering::SeqCst);
            let (result, unchanged) = tokio::join!(
                tokio::time::timeout(Duration::from_secs(3), fixture.client.adopt_home(&claim)),
                async {
                    if prior_pending {
                        entered(&first).await;
                        first.release.notify_one();
                    }
                    entered(&failure).await;
                    let unchanged = fixture.state.lock().unwrap().rows.clone();
                    failure.release.notify_one();
                    unchanged
                }
            );
            assert!(matches!(result.unwrap(), Err(AdapterError::Unavailable(_))));
            assert_eq!(fixture.state.lock().unwrap().rows, unchanged);
            let authority = authority_tests::published(&fixture, &claim);
            authority_tests::retry(&fixture, &claim, &authority).await;
        }
    }
}

#[tokio::test]
async fn initial_or_pending_then_failed_claim_read_keeps_transport_error() {
    transport_failure("claim").await;
}

#[tokio::test]
async fn initial_or_pending_then_failed_member_read_keeps_transport_error() {
    transport_failure("member").await;
}

#[tokio::test]
async fn initial_or_pending_then_failed_pvc_read_keeps_transport_error() {
    transport_failure("pvc").await;
}

#[tokio::test]
async fn repeated_http_failures_in_guard_cannot_reuse_prior_pending() {
    for prior_pending in [false, true] {
        let (fixture, claim) = fixture(true).await;
        if prior_pending {
            let mut state = fixture.state.lock().unwrap();
            let pod = state.rows.get_mut(super::POD).unwrap();
            pod["status"]["conditions"][0]["status"] = "False".into();
            pod["status"]["containerStatuses"][0]["ready"] = false.into();
        }
        let before = fixture.state.lock().unwrap().rows.clone();
        fixture.state.lock().unwrap().calls.clear();
        let first = prior_pending
            .then(|| fixture.gate("GET /api/v1/namespaces/cma-test/pods".into(), None));
        let failures = || {
            fixture
                .state
                .lock()
                .unwrap()
                .lose
                .extend(std::iter::repeat_n(
                    format!("GET {}", path("claim", &claim)),
                    100,
                ));
        };
        if !prior_pending {
            failures();
        }
        let (result, ()) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(3), fixture.client.adopt_home(&claim)),
            async {
                if let Some(first) = first {
                    entered(&first).await;
                    failures();
                    first.release.notify_one();
                }
            }
        );
        assert_eq!(
            result.unwrap().unwrap_err(),
            AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut)
        );
        let mut state = fixture.state.lock().unwrap();
        assert_eq!(state.rows, before);
        assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
        assert!(
            state
                .calls
                .iter()
                .filter(|(call, _)| call.contains("sandboxclaims/"))
                .count()
                >= 2
        );
        state.lose.clear();
    }
}
