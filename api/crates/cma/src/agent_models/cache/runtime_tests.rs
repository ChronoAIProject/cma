use super::*;
use layer2_core::sandbox::{RuntimeBinding, SandboxHome};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};
use std::sync::atomic::Ordering;
#[path = "test_runtime.rs"]
mod test_runtime;
use test_runtime::Probe;

fn fixture() -> (Sandbox, RuntimeStatus, Arc<Probe>) {
    let mut root = crate::store::agent_access_fixtures::sandbox("support-cache");
    let mut home = SandboxHome::reserve(
        &root.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-original".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home".into(),
        storage_generation: "original".into(),
    })
    .unwrap();
    let status: RuntimeStatus = serde_json::from_value(serde_json::json!({
        "sandbox_id": root.id, "generation": 1,
        "home_identity": home.attachment().unwrap().identity().unwrap(),
        "boot_id": "a".repeat(32), "installation_ready": true,
        "credential_ready": true, "restore_pending": false, "agent_processes": 1,
        "agent_model_catalog": true,
    }))
    .unwrap();
    root.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-fixture".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        boot_id: Some(status.boot_id.clone()),
        bound_at: root.created_at.clone(),
    });
    root.lifecycle.runtime_generation_high_water = 1;
    root.lifecycle.home = Some(Box::new(home));
    root.validate().unwrap();
    let probe = Arc::new(Probe::default());
    *probe.status.lock().unwrap() = Some(status.clone());
    (root, status, probe)
}

#[tokio::test]
async fn support_cache_coalesces_and_revalidates_exact_runtime_identity() {
    let (root, status, probe) = fixture();
    let support = CachedModelSupport::wrap(probe.clone(), true);
    let values = futures_util::future::join_all((0..200).map(|_| support.supported(&root))).await;
    assert!(values.into_iter().all(|value| value));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    for field in 0..4 {
        let mut changed = root.clone();
        match field {
            0 => changed.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32)),
            1 => {
                changed.runtime.as_mut().unwrap().provision_generation = Some(2);
                changed.lifecycle.runtime_generation_high_water = 2;
            }
            2 => changed.lifecycle.home = None,
            _ => changed.id = layer2_core::SandboxId("other".into()),
        }
        assert!(!support.supported(&changed).await);
    }
    // A valid unsupported observation is distinct from a failed transport.
    let mut next = root.clone();
    next.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    let mut unsupported = status;
    unsupported.boot_id = "b".repeat(32);
    unsupported.agent_model_catalog = false;
    *probe.status.lock().unwrap() = Some(unsupported);
    assert!(!support.supported(&next).await);
    let before = probe.calls.load(Ordering::SeqCst);
    assert!(!support.supported(&next).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), before);
}

#[tokio::test(start_paused = true)]
async fn support_failure_timeout_unknown_boot_and_disabled_cache_do_not_reuse_success() {
    let (root, status, probe) = fixture();
    let support = CachedModelSupport::wrap(probe.clone(), true);
    *probe.status.lock().unwrap() = None;
    assert!(!support.supported(&root).await);
    *probe.status.lock().unwrap() = Some(status);
    probe.delay_ms.store(6000, Ordering::SeqCst);
    assert!(!support.supported(&root).await);
    probe.delay_ms.store(0, Ordering::SeqCst);
    assert!(support.supported(&root).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
    let mut unbound = root.clone();
    unbound.runtime.as_mut().unwrap().boot_id = None;
    assert!(support.supported(&unbound).await);
    assert!(support.supported(&unbound).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 5);
    let disabled = CachedModelSupport::wrap(probe.clone(), false);
    assert!(disabled.supported(&root).await);
    assert!(disabled.supported(&root).await);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 7);
}

#[tokio::test(start_paused = true)]
async fn concurrent_stalled_support_reads_do_not_serialize_timeouts() {
    let (root, _, probe) = fixture();
    probe.delay_ms.store(6000, Ordering::SeqCst);
    let support = CachedModelSupport::wrap(probe, true);
    let started = tokio::time::Instant::now();
    let values = futures_util::future::join_all((0..200).map(|_| support.supported(&root))).await;
    assert!(values.into_iter().all(|value| !value));
    assert_eq!(started.elapsed(), Duration::from_secs(5));
}
