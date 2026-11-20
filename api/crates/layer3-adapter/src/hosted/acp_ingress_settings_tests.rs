use super::recovery_tests::{next_request, reply};
use super::tests::fixture;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedConversationSettingsSupport, AllocationPlan, IngressClass,
};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn receipt_query_uses_the_admitted_source_only_after_local_readiness() {
    let mut fixture = fixture();
    let (binding, source) =
        crate::hosted::managed_context::receipt_admission::tests::binding(&fixture.policy, 0, None);
    fixture.bridge.context_refresh = Some(binding);
    fixture.activate().await;
    let result = fixture
        .bridge
        .managed_context_receipt(7, &fixture.policy)
        .unwrap()
        .await
        .unwrap();
    assert!(result.is_none());
    drop(result);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    let unsupported_goal = fixture
        .bridge
        .managed_transport_receipt(7, &fixture.policy)
        .unwrap()
        .await
        .unwrap_err();
    assert!(matches!(
        unsupported_goal,
        crate::ingress_budget::AdmittedError::Unsupported(_)
    ));
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    fixture
        .bridge
        .observe_context_goal(
            "conversation",
            &json!({
                "sessionUpdate":"session_info_update", "_meta":{"goal":null}
            }),
        )
        .await;
    let first = fixture
        .bridge
        .managed_context_receipt(7, &fixture.policy)
        .unwrap()
        .await
        .unwrap();
    let transport = fixture
        .bridge
        .managed_transport_receipt(7, &fixture.policy)
        .unwrap()
        .await
        .unwrap();
    assert!(first.is_some());
    assert_eq!(*first, *transport);
    assert!(first.belongs_to(&fixture.policy));
    assert!(transport.belongs_to(&fixture.policy));
    assert_eq!(source.calls.load(Ordering::SeqCst), 3);
    for (sequence, busy) in [(0, false), (7, true)] {
        fixture
            .bridge
            .owned_prompt_waiters
            .store(usize::from(busy), Ordering::SeqCst);
        let result = fixture
            .bridge
            .managed_context_receipt(sequence, &fixture.policy)
            .unwrap()
            .await
            .unwrap();
        assert!(result.is_none());
    }
    assert_eq!(source.calls.load(Ordering::SeqCst), 3);
    let root = fixture.root.clone();
    drop((fixture, source));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop((first, transport));
    root.assert_idle();
}

#[tokio::test]
async fn receipt_query_refuses_an_unsupported_source_without_legacy_io() {
    let mut fixture = fixture();
    let (binding, files) =
        crate::hosted::managed_context::writer_tests::owned_binding(&fixture.policy, 0);
    fixture.bridge.context_refresh = Some(binding);
    fixture.activate().await;
    fixture
        .bridge
        .observe_context_goal(
            "conversation",
            &json!({
                "sessionUpdate":"session_info_update", "_meta":{"goal":null}
            }),
        )
        .await;
    let error = fixture
        .bridge
        .managed_context_receipt(1, &fixture.policy)
        .unwrap()
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::ingress_budget::AdmittedError::Unsupported(_)
    ));
    assert_eq!(files.status_reads.load(Ordering::SeqCst), 0);
    assert_eq!(files.goal_probes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn instruction_query_preserves_reply_validation_and_cached_result() {
    for supported in [true, false] {
        let fixture = fixture();
        fixture.activate().await;
        let operation = fixture
            .bridge
            .system_prompt_supported(&fixture.policy)
            .unwrap();
        let peer = async {
            let request = next_request(&fixture).await;
            assert_eq!(request["method"], "cma/agent-instructions");
            assert_eq!(request["params"], json!({"action":"support"}));
            reply(
                &fixture,
                &request,
                json!({"version":1,"supported":supported}),
            )
            .await;
        };
        let (value, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(operation, peer)
        })
        .await
        .unwrap();
        let value = value.unwrap();
        assert_eq!(*value, supported);
        assert!(value.belongs_to(&fixture.policy));
        assert_eq!(
            fixture.bridge.instructions_support.load(Ordering::Acquire),
            if supported { 1 } else { 2 }
        );
        let cached = fixture
            .bridge
            .system_prompt_supported(&fixture.policy)
            .unwrap()
            .await
            .unwrap();
        assert_eq!(*cached, supported);
        assert!(fixture.writes.lock().unwrap().is_empty());
        let root = fixture.root.clone();
        drop(fixture);
        assert!(root.available_bytes() < root.capacity_bytes());
        drop((value, cached));
        root.assert_idle();
    }
}

#[tokio::test]
async fn local_readiness_retains_existing_negative_only_evidence_without_source_io() {
    let mut fixture = fixture();
    let (binding, files) =
        crate::hosted::managed_context::writer_tests::owned_binding(&fixture.policy, 0);
    fixture.bridge.context_refresh = Some(binding);
    fixture.activate().await;
    let hint = fixture
        .bridge
        .managed_context_may_be_ready(&fixture.policy)
        .unwrap()
        .await
        .unwrap();
    assert!(!*hint);
    drop(hint);
    fixture
        .bridge
        .observe_context_goal(
            "conversation",
            &json!({
                "sessionUpdate":"session_info_update", "_meta":{"goal":null}
            }),
        )
        .await;
    for busy in [false, true, false] {
        fixture
            .bridge
            .owned_prompt_waiters
            .store(usize::from(busy), Ordering::SeqCst);
        let baseline = fixture.root.available_bytes();
        let value = fixture
            .bridge
            .managed_context_may_be_ready(&fixture.policy)
            .unwrap()
            .await
            .unwrap();
        assert_eq!(*value, !busy);
        assert_eq!(
            *value,
            crate::ConversationSettingsSupport::managed_context_may_be_ready(&fixture.bridge).await
        );
        assert!(value.belongs_to(&fixture.policy));
        drop(value);
        assert_eq!(fixture.root.available_bytes(), baseline);
    }
    assert_eq!(files.status_reads.load(Ordering::SeqCst), 0);
    assert_eq!(files.goal_probes.load(Ordering::SeqCst), 0);
    assert!(fixture.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn query_refusal_precedes_bridge_work_and_source_io() {
    let mut fixture = fixture();
    let (binding, files) =
        crate::hosted::managed_context::writer_tests::owned_binding(&fixture.policy, 0);
    fixture.bridge.context_refresh = Some(binding);
    fixture.activate().await;
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let pressure = fixture
        .policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("query pressure", &[available]).unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    for _ in 0..128 {
        let error = probe.track(|| {
            fixture
                .bridge
                .system_prompt_supported(&fixture.policy)
                .unwrap_err()
        });
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::AtCapacity { .. })
        ));
        let error = probe.track(|| {
            fixture
                .bridge
                .managed_context_may_be_ready(&fixture.policy)
                .unwrap_err()
        });
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::AtCapacity { .. })
        ));
        for error in [
            probe.track(|| {
                fixture
                    .bridge
                    .managed_context_receipt(1, &fixture.policy)
                    .unwrap_err()
            }),
            probe.track(|| {
                fixture
                    .bridge
                    .managed_transport_receipt(1, &fixture.policy)
                    .unwrap_err()
            }),
        ] {
            assert!(matches!(
                error.refusal(),
                Some(AdmissionRefusal::AtCapacity { .. })
            ));
        }
    }
    assert_eq!(probe.allocated(), 0);
    assert_eq!(
        fixture.bridge.instructions_support.load(Ordering::Acquire),
        0
    );
    assert_eq!(files.status_reads.load(Ordering::SeqCst), 0);
    assert_eq!(files.goal_probes.load(Ordering::SeqCst), 0);
    assert!(fixture.writes.lock().unwrap().is_empty());
    drop((probe, pressure));
}

#[tokio::test]
async fn foreign_policy_cannot_query_the_bridge() {
    let fixture = fixture();
    let foreign = super::tests::fixture();
    for error in [
        fixture
            .bridge
            .system_prompt_supported(&foreign.policy)
            .unwrap_err(),
        fixture
            .bridge
            .managed_context_may_be_ready(&foreign.policy)
            .unwrap_err(),
        fixture
            .bridge
            .managed_context_receipt(1, &foreign.policy)
            .unwrap_err(),
        fixture
            .bridge
            .managed_transport_receipt(1, &foreign.policy)
            .unwrap_err(),
    ] {
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::InvalidPlan { .. })
        ));
    }
    assert!(fixture.writes.lock().unwrap().is_empty());
}

#[test]
fn isolated_readiness_query_admits_the_maximum_identity_before_copying() {
    use futures_util::FutureExt;
    const TEST: &str = "hosted::acp::ingress::settings_tests::isolated_readiness_query_admits_the_maximum_identity_before_copying";
    if std::env::var_os("CMA_SETTINGS_QUERY_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_SETTINGS_QUERY_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let mut fixture = fixture();
    fixture.bridge.context_refresh =
        Some(crate::hosted::managed_context::writer_tests::owned_binding(&fixture.policy, 0).0);
    let session = "s".repeat(crate::hosted::acp_session_identity::MAX_BYTES);
    let (relay, capture) =
        crate::hosted::update_relay::UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    fixture
        .bridge
        .activate_session(session.clone(), &relay)
        .now_or_never()
        .unwrap()
        .unwrap();
    fixture
        .bridge
        .observe_context_goal(
            &session,
            &json!({
                "sessionUpdate":"session_info_update", "_meta":{"goal":null}
            }),
        )
        .now_or_never()
        .unwrap();
    let baseline = fixture.root.available_bytes();
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        fixture
            .bridge
            .managed_context_may_be_ready(&fixture.policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    let admitted = baseline - fixture.root.available_bytes();
    assert!(*result);
    assert!(probe.peak() >= session.len());
    assert!(
        probe.peak() <= admitted,
        "settings peak={} retained={} admitted={admitted}",
        probe.peak(),
        probe.live()
    );
    eprintln!(
        "settings_query peak={} retained={} admitted={admitted}",
        probe.peak(),
        probe.live()
    );
    drop(result);
    assert_eq!(probe.live(), 0);
    assert_eq!(fixture.root.available_bytes(), baseline);
    drop(probe);

    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let pressure = fixture
        .policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("query pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let mut probe = AllocationProbe::new();
    let legacy = probe.track(|| {
        crate::ConversationSettingsSupport::managed_context_may_be_ready(&fixture.bridge)
            .now_or_never()
            .unwrap()
    });
    assert!(legacy);
    assert!(
        probe.peak() >= session.len(),
        "legacy query still copies under pressure"
    );
    eprintln!(
        "legacy_settings_query_under_pressure peak={} retained={}",
        probe.peak(),
        probe.live()
    );
    drop(probe);
    let mut probe = AllocationProbe::new();
    let refused = probe.track(|| {
        fixture
            .bridge
            .managed_context_may_be_ready(&fixture.policy)
            .unwrap_err()
    });
    assert!(matches!(
        refused.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    drop((probe, pressure));
    assert_eq!(fixture.root.available_bytes(), baseline);
    let root = fixture.root.clone();
    drop((fixture, relay, capture));
    root.assert_idle();
}
