use super::*;
use crate::sandbox::warmup::prepare_environment;
use layer2_core::sandbox::{ProvisioningExecution, WarmSandbox, WarmSandboxPlan, WarmSandboxState};

struct WarmHome(layer3_adapter::HomeVolumeAttachment);

#[async_trait::async_trait]
impl layer3_adapter::HomeVolumeStore for WarmHome {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<layer3_adapter::HomeVolumeAttachment>> {
        Ok(vec![self.0.clone()])
    }
    async fn prepare(
        &self,
        _: &layer3_adapter::HomeVolumeRequest,
    ) -> layer3_adapter::Result<layer3_adapter::HomeVolumeReceipt> {
        panic!("the warm fixture already has a recorded home")
    }
    async fn verify(
        &self,
        home: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        assert_eq!(home, &self.0);
        Ok(())
    }
    async fn remove(
        &self,
        _: &layer3_adapter::HomeVolumeRequest,
        _: Option<&layer3_adapter::HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("this campaign observes retirement admission only")
    }
}

async fn persistent_warm(harness: &Harness) -> (Arc<dyn SandboxAdapter>, WarmSandbox) {
    let candidate = harness.create().await;
    let mut reaches = candidate.credential.reaches.clone();
    reaches.sort();
    let mut record = WarmSandbox::new(
        WarmSandboxPlan {
            profile: candidate.profile.clone(),
            reaches,
            runtime_version: runtime_image(),
            execution: ProvisioningExecution::default(),
        },
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    let mut home = layer2_core::sandbox::SandboxHome::reserve(
        &record.id,
        layer3_adapter::HomeVolumePolicy {
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "original-seed".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "warm-original".into(),
        storage_generation: "warm-volume-original".into(),
    })
    .unwrap();
    let homes = Arc::new(WarmHome(home.attachment().unwrap()));
    record.home = Some(Box::new(home));
    let adapter: Arc<dyn SandboxAdapter> = Arc::new(
        CodexAdapter::with_backend(harness.backend.clone(), CodexSandboxSpec::default())
            .with_home_volumes(homes),
    );
    let installing = prepare_environment(adapter.as_ref(), "test", &record, None)
        .await
        .unwrap();
    assert_eq!(installing.state, WarmSandboxState::Installing);
    assert!(installing.runtime.as_ref().unwrap().boot_id.is_some());
    let ready = prepare_environment(adapter.as_ref(), "test", &installing, None)
        .await
        .unwrap();
    ready.validate().unwrap();
    assert_eq!(ready.state, WarmSandboxState::Ready);
    (adapter, ready)
}

#[tokio::test]
async fn restarted_warm_process_is_retired_even_when_provider_handle_is_still_running() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (adapter, ready) = persistent_warm(&harness).await;
    let handle = RuntimeHandle::new(&ready.runtime.as_ref().unwrap().runtime_id);
    let original = adapter.runtime_status(&handle).await.unwrap();
    assert!(original.installation_ready);
    assert!(!original.credential_ready);
    harness
        .backend
        .restart_persistent_process(&handle)
        .await
        .unwrap();
    assert!(adapter.runtime_inventory().await.unwrap()[0]
        .lifecycle
        .is_running());
    let checked = prepare_environment(adapter.as_ref(), "test", &ready, None)
        .await
        .unwrap();
    assert_eq!(checked.state, WarmSandboxState::Retiring);
    assert_eq!(checked.runtime, ready.runtime);
    assert!(
        !adapter
            .runtime_status(&handle)
            .await
            .unwrap()
            .credential_ready
    );
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn credentialed_warm_process_is_never_republished_as_available() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (adapter, ready) = persistent_warm(&harness).await;
    let handle = RuntimeHandle::new(&ready.runtime.as_ref().unwrap().runtime_id);
    adapter
        .deliver_credential(
            &handle,
            layer3_adapter::CredentialFrame {
                operation_id: "fixture-contamination".into(),
                value: layer3_adapter::RuntimeCredential::new("opaque-warm-fixture"),
                custody_generation: Some(1),
            },
        )
        .await
        .unwrap();
    let checked = prepare_environment(adapter.as_ref(), "test", &ready, None)
        .await
        .unwrap();
    assert_eq!(checked.state, WarmSandboxState::Retiring);
    assert_eq!(checked.runtime, ready.runtime);
}

async fn prepare(harness: &Harness, candidate: &Sandbox) -> WarmSandbox {
    let mut reaches = candidate.credential.reaches.clone();
    reaches.sort();
    let record = WarmSandbox::new(
        WarmSandboxPlan {
            profile: candidate.profile.clone(),
            reaches,
            runtime_version: runtime_image(),
            execution: ProvisioningExecution::default(),
        },
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    let installing = prepare_environment(harness.adapter.as_ref(), "test", &record, None)
        .await
        .unwrap();
    assert_eq!(installing.state, WarmSandboxState::Installing);
    let ready = prepare_environment(harness.adapter.as_ref(), "test", &installing, None)
        .await
        .unwrap();
    ready.validate().unwrap();
    assert_eq!(ready.state, WarmSandboxState::Ready);
    ready
}

#[tokio::test]
async fn warm_install_precedes_credentials_and_admission_reuses_exact_runtime() {
    let harness = Harness::new(vec!["install test-tool".into()], 0, Duration::from_secs(60)).await;
    let mut candidate = harness.create().await;
    candidate.name.push_str("-warm");
    let mut ready = prepare(&harness, &candidate).await;
    let id = ready.id.clone();
    let handle = RuntimeHandle::new(&ready.runtime.as_ref().unwrap().runtime_id);
    assert!(
        !harness
            .backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
    assert!(harness.actions.snapshot().is_empty());
    ready
        .claim(
            &mut candidate,
            &runtime_image(),
            &layer2_core::now_rfc3339(),
        )
        .unwrap();
    harness.store.create_sandbox(candidate).await.unwrap();
    let active = harness
        .drive_active(&id, SystemTime::now() + Duration::from_secs(1))
        .await;
    assert_eq!(active.runtime.as_ref().unwrap().runtime_id, handle.as_str());
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
    assert!(
        harness
            .backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
    let requests = harness.backend.requests().await;
    let installs: Vec<_> = requests
        .iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::Install(request) => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(
        installs.len(),
        2,
        "the owned worker checks the original install receipt"
    );
    assert_eq!(
        installs[0], installs[1],
        "installation replay has exactly the same identity"
    );
}

#[tokio::test]
async fn lost_warm_runtime_is_replaced_before_any_user_credential_is_delivered() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let mut candidate = harness.create().await;
    candidate.name.push_str("-warm");
    let mut ready = prepare(&harness, &candidate).await;
    let id = ready.id.clone();
    ready
        .claim(
            &mut candidate,
            &runtime_image(),
            &layer2_core::now_rfc3339(),
        )
        .unwrap();
    harness.store.create_sandbox(candidate).await.unwrap();
    let old = harness
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .pop()
        .unwrap();
    harness.adapter.reclaim_runtime(&old).await.unwrap();
    crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        harness.adapter.clone(),
        Arc::new(harness.policy.clone()),
        "test".into(),
        runtime_image(),
    )
    .initialize(SystemTime::now())
    .await
    .unwrap();
    assert_eq!(
        harness.stored(&id).await.status,
        SandboxStatus::Provisioning
    );
    let active = harness
        .drive_active(&id, SystemTime::now() + Duration::from_secs(1))
        .await;
    assert_eq!(active.id, id);
    let binding = active.runtime.as_ref().unwrap();
    assert_ne!(binding.runtime_id, old.handle.as_str());
    assert_eq!(binding.provision_generation, Some(2));
    assert_eq!(harness.adapter.runtime_inventory().await.unwrap().len(), 1);
    assert!(
        harness
            .backend
            .snapshot(&RuntimeHandle::new(&binding.runtime_id))
            .await
            .unwrap()
            .credential_delivered
    );
}

#[tokio::test]
async fn installation_failure_never_becomes_ready_or_obtains_a_user_key() {
    let harness = Harness::new(vec!["install test-tool".into()], 0, Duration::from_secs(60)).await;
    let candidate = harness.create().await;
    let mut reaches = candidate.credential.reaches.clone();
    reaches.sort();
    let record = WarmSandbox::new(
        WarmSandboxPlan {
            profile: candidate.profile.clone(),
            reaches,
            runtime_version: runtime_image(),
            execution: ProvisioningExecution::default(),
        },
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    let installing = prepare_environment(harness.adapter.as_ref(), "test", &record, None)
        .await
        .unwrap();
    harness.backend.corrupt_next_install_receipt();
    assert!(
        prepare_environment(harness.adapter.as_ref(), "test", &installing, None)
            .await
            .is_err()
    );
    assert!(harness.actions.snapshot().is_empty());
    let handle = RuntimeHandle::new(&installing.runtime.as_ref().unwrap().runtime_id);
    assert!(
        !harness
            .backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
}

#[tokio::test]
async fn changed_identity_or_incomplete_install_cannot_be_claimed() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let candidate = harness.create().await;
    let ready = prepare(&harness, &candidate).await;
    for variant in 0..4 {
        let mut changed = candidate.clone();
        let mut reservation = ready.clone();
        let mut image = runtime_image();
        match variant {
            0 => changed.profile.id = SandboxProfileId::generate(),
            1 => changed
                .credential
                .reaches
                .push(ServiceId::new("additional-service")),
            2 => image.push_str("-changed"),
            _ => reservation.state = WarmSandboxState::Installing,
        }
        let before = changed.clone();
        assert!(reservation
            .claim(&mut changed, &image, &layer2_core::now_rfc3339())
            .is_err());
        assert_eq!(changed, before);
    }
}
