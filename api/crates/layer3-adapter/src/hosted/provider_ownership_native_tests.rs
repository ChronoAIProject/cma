use super::super::*;
use crate::hosted::HostedControlTransport;
use crate::owner_protocol::{
    observe_inspection_failure, scope, test_support::TestAuthority, OwnershipFailure,
    OwnershipFailureReason as Reason, OwnershipPhase as Phase, StagingInspectionRefusal,
    TransportAuthority,
};

#[path = "provider_ownership_retry_native_tests.rs"]
mod retry_evidence;

pub(super) async fn respond(
    stream: &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin),
    request: &str,
    state: &State,
    fault: usize,
) -> bool {
    let phase = match fault / 1000 {
        1 => "challenge",
        2 => "confirm",
        3 => "revoke",
        _ => panic!("unknown diagnostic fault"),
    };
    if !request
        .lines()
        .next()
        .unwrap()
        .contains(&format!("/manager-ownership/{phase} "))
    {
        state.fault.store(fault, Ordering::SeqCst);
        return false;
    }
    match fault % 1000 {
        1 => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        }
        2 => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}")
                .await
                .unwrap();
        }
        3 => {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4097\r\n\r\n")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        }
        4 => {
            let body = "x".repeat(4097);
            stream.write_all(format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1001\r\n{body}\r\n0\r\n\r\n").as_bytes()).await.unwrap();
        }
        5..=8 => {
            let code = fault % 1000;
            let body = serde_json::json!({"protocol": if code == 8 { 1 } else { 2 },
                "nonce": if code == 6 { "malformed" } else { "00000000-0000-4000-8000-000000000001" },
                "valid_for_ms": if code == 8 { 20000 } else { 10000 }}).to_string();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        status => {
            let body = "private-native-body-sentinel";
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Refused\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    }
    true
}

async fn fixture() -> Fixture {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"a".repeat(64))
        .await
        .unwrap();
    fixture
}

async fn refused(phase: Phase, code: usize, expected: OwnershipFailure) {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    assert!(
        endpoint.native_binding().is_some(),
        "must exercise the actual native HTTP branch"
    );
    let bucket = match phase {
        Phase::Challenge => 1,
        Phase::Confirm => 2,
        _ => unreachable!(),
    };
    fixture
        .state
        .fault
        .store(bucket * 1000 + code, Ordering::SeqCst);
    let authority = TestAuthority::new(7);
    assert!(scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint)
    )
    .await
    .is_err());
    assert_eq!(authority.failure.get(), Some(&expected));
    assert!(authority.failure_before_revocation.load(Ordering::SeqCst));
    assert!(!authority.is_current());
    assert_eq!(
        fixture.state.ownership_confirmations.load(Ordering::SeqCst),
        0
    );
    let requests = fixture.state.requests.lock().unwrap().clone();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.lines().next().unwrap().contains("/confirm "))
            .count(),
        usize::from(phase == Phase::Confirm)
    );
    assert!(!requests
        .iter()
        .any(|request| request.starts_with("POST /runtime/control ")));
    let receivers = authority.receivers.lock().unwrap().clone();
    assert_eq!(receivers.len(), 1);
    assert!(scope(
        authority.clone(),
        fixture
            .transport
            .exchange(&handle(), RequestFrame::Resources)
    )
    .await
    .is_err());
    assert_eq!(
        *fixture.state.requests.lock().unwrap(),
        requests,
        "cancelled authority cannot send another control"
    );
    fixture.state.fault.store(3403, Ordering::SeqCst);
    assert!(receivers[0].revoke().await.is_err());
    assert_eq!(
        authority.failure.get(),
        Some(&expected),
        "cleanup must retain the first cause"
    );
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_ownership_http_refusals_keep_status_and_block_confirmation_and_control() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        for status in [401, 403, 404, 409, 503] {
            refused(phase, status, OwnershipFailure::http(phase, status as u16)).await;
        }
    }
}

#[tokio::test]
async fn staging_inspection_refusal_reaches_the_observer_and_expires_on_the_same_binding() {
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let authority = TestAuthority::new(7);
    let observed = Arc::new(OnceLock::new());
    let slot = observed.clone();
    let runtime = handle();
    let expiry = SystemTime::now() + Duration::from_secs(60);
    let fail = scope(
        authority.clone(),
        observe_inspection_failure(
            move |refusal| assert!(slot.set(refusal).is_ok()),
            Some(StagingInspectionRefusal::new(
                runtime.as_str().into(),
                expiry,
            )),
            fixture.transport.receiver_lease(&endpoint),
        ),
    )
    .await;
    assert!(fail.is_err());
    let refusal = observed
        .get()
        .expect("the parking observer receives the refusal");
    assert_eq!(refusal.owner, authority.owner());
    assert_eq!(refusal.receiver.runtime_id(), runtime.as_str());
    assert_eq!(
        refusal.failure,
        OwnershipFailure::http(Phase::Challenge, 503)
    );
    assert!(!authority.is_current());
    assert!(fixture
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| {
            !request
                .lines()
                .next()
                .unwrap()
                .contains("/manager-ownership/challenge ")
        }));
    assert_eq!(
        fixture.state.ownership_confirmations.load(Ordering::SeqCst),
        0
    );

    for (id, time) in [
        ("different-runtime", expiry),
        (runtime.as_str(), UNIX_EPOCH),
    ] {
        let owner = TestAuthority::new(8);
        let lease = scope(
            owner,
            observe_inspection_failure(
                |_| panic!("a real receiver succeeds outside the active fixture scope"),
                Some(StagingInspectionRefusal::new(id.into(), time)),
                fixture.transport.receiver_lease(&endpoint),
            ),
        )
        .await
        .unwrap();
        assert!(lease.is_some());
    }
    assert_eq!(
        fixture.state.ownership_confirmations.load(Ordering::SeqCst),
        2
    );
    fixture.transport.retire(&runtime).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_ownership_body_deadline_preserves_the_received_status() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        refused(
            phase,
            1,
            OwnershipFailure::new(phase, Reason::Deadline).with_status(200),
        )
        .await;
    }
}

#[tokio::test]
async fn native_ownership_body_reset_preserves_the_received_status() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        refused(
            phase,
            2,
            OwnershipFailure::new(phase, Reason::TransportUnavailable).with_status(200),
        )
        .await;
    }
}

#[tokio::test]
async fn native_ownership_oversized_bodies_are_malformed_with_the_received_status() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        for code in [3, 4] {
            refused(
                phase,
                code,
                OwnershipFailure::new(phase, Reason::MalformedReply).with_status(200),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn acting_replica_repeats_exact_retirement_challenge_without_confirming() {
    for mode in [
        NativeRuntimeTransport::KubernetesExec,
        NativeRuntimeTransport::KubernetesPortforward,
    ] {
        let f = Fixture::start(Mode::Persistent, mode).await;
        let endpoint = f.endpoint().await;
        let (_, binding) = endpoint.native_binding().unwrap();
        let authority = TestAuthority::new(3);
        f.state.fault.store(1404, Ordering::SeqCst);
        let (result, observation) = crate::receiver_failure::observe(scope(
            authority.clone(),
            f.transport.receiver_lease(&endpoint),
        ))
        .await;
        assert!(result.is_err());
        let captured = observation.unwrap().into_receiver().unwrap();
        assert_eq!(captured.runtime, handle());
        assert_eq!(
            captured.pod,
            crate::RetirementPod::try_from(binding).unwrap()
        );
        assert_eq!(captured.owner, authority.owner());

        let RuntimeDiscovery::AgentSandbox(client) = &f.transport.client else {
            unreachable!()
        };
        let acting = ProviderHostedControlTransport::for_agent_sandbox_with_transport(
            (**client).clone(),
            mode,
        )
        .unwrap();
        for (fault, expected) in [
            (1404, Some(true)),
            (1403, None),
            (2, None),
            (0, Some(false)),
        ] {
            f.state.fault.store(fault, Ordering::SeqCst);
            assert_eq!(
                acting
                    .challenge_native_receiver(&handle(), binding, &captured.owner)
                    .await
                    .ok(),
                expected,
            );
        }
        f.state.restarted.store(true, Ordering::SeqCst);
        let before = f.state.requests.lock().unwrap().len();
        assert!(acting
            .challenge_native_receiver(&handle(), binding, &captured.owner)
            .await
            .is_err());
        let requests = f.state.requests.lock().unwrap();
        assert_eq!(requests.len(), before);
        for request in requests.iter() {
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers
                .lines()
                .next()
                .unwrap()
                .contains("/manager-ownership/challenge "));
            assert_eq!(
                serde_json::from_str::<crate::owner_protocol::TransportOwner>(body).unwrap(),
                captured.owner,
            );
        }
        assert_eq!(authority.renewals.load(Ordering::SeqCst), 0);
        assert_eq!(f.state.ownership_confirmations.load(Ordering::SeqCst), 0);
    }
}
