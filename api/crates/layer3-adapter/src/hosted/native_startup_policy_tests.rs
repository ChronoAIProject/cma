use super::*;
use crate::hosted::HostedControlTransport;

fn request() -> crate::RuntimeStatusRequest {
    crate::RuntimeStatusRequest {
        sandbox_id: "sandbox-one".into(),
        generation: 1,
        home_identity: "a".repeat(43),
    }
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

#[tokio::test]
async fn native_startup_policy_requires_negotiation_and_the_observed_boot() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let (_, pod) = endpoint.native_binding().unwrap();
    let handle = handle();
    let request = request();
    let boot = "a".repeat(32);
    let policy = fixture
        .transport
        .observe_native_startup_policy(&handle, pod, &request, &boot)
        .await
        .unwrap();
    assert!(policy.is_none());

    for enabled in [false, true] {
        *fixture.state.startup_policy.lock().unwrap() = Some(enabled);
        let policy = fixture
            .transport
            .observe_native_startup_policy(&handle, pod, &request, &boot)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(policy.runtime.boot_id, boot);
        assert_eq!(policy.legacy_journal_recovery_enabled, enabled);
    }

    assert!(matches!(
        fixture
            .transport
            .observe_native_startup_policy(&handle, pod, &request, &"b".repeat(32))
            .await,
        Err(AdapterError::Rejected(_))
    ));

    let reply = fixture
        .transport
        .exchange(&handle, RequestFrame::RuntimeStatus(request))
        .await
        .unwrap();
    assert!(matches!(reply, ResponseFrame::RuntimeStatus(_)));
    fixture.transport.retire(&handle).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn native_startup_policy_rejects_a_container_restart_during_the_read() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let (_, pod) = endpoint.native_binding().unwrap();
    *fixture.state.startup_policy.lock().unwrap() = Some(true);
    fixture.state.hold_response.store(true, Ordering::SeqCst);

    let transport = fixture.transport.clone();
    let pod = pod.clone();
    let observation = tokio::spawn(async move {
        transport
            .observe_native_startup_policy(&handle(), &pod, &request(), &"a".repeat(32))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("status request reached the original container");
    fixture.state.restarted.store(true, Ordering::SeqCst);
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    assert!(matches!(
        observation.await.unwrap(),
        Err(AdapterError::Rejected(_))
    ));
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn retained_startup_policy_requires_effective_false_on_the_exact_running_boot() {
    let fixture = fixture().await;
    fixture
        .state
        .retained_evidence
        .store(true, Ordering::SeqCst);
    let request = request();
    let boot = "a".repeat(32);
    let changed_boot = "b".repeat(32);
    let image = format!("registry.example/runtime@sha256:{}", "d".repeat(64));
    let config = format!("sha256:{}", "c".repeat(64));
    let wrong_config = format!("sha256:{}", "e".repeat(64));
    let mut readback = crate::RetainedStartupPolicyReadback {
        runtime: &request,
        boot_id: &boot,
        image: &image,
        config_digest: &config,
        journal_recovery: false,
    };
    let handle = handle();
    let transport = &fixture.transport;
    assert!(matches!(
        transport.verify_retained_policy(&handle, &readback).await,
        Err(AdapterError::Unavailable(message))
            if message == "admitted runtime startup policy is not observable"
    ));
    *fixture.state.startup_policy.lock().unwrap() = Some(false);
    readback.boot_id = &changed_boot;
    assert!(matches!(
        transport.verify_retained_policy(&handle, &readback).await,
        Err(AdapterError::Rejected(message))
            if message == "runtime container or boot changed during startup policy observation"
    ));
    readback.boot_id = &boot;
    readback.config_digest = &wrong_config;
    assert!(matches!(
        transport.verify_retained_policy(&handle, &readback).await,
        Err(AdapterError::Unavailable(message))
            if message == "admitted runtime image is not observed"
    ));
    readback.config_digest = &config;
    let positive = transport
        .verify_retained_policy(&handle, &readback)
        .await
        .unwrap();
    assert_eq!(positive.runtime.boot_id, boot);
    assert!(!positive.legacy_journal_recovery_enabled);
    transport.retire(&handle).await.unwrap();
    fixture.drained().await;
}
