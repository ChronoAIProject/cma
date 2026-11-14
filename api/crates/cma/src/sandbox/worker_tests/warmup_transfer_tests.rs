use super::*;
use crate::sandbox::warmup::prepare_environment;
use layer2_core::sandbox::{ProvisioningExecution, WarmSandbox, WarmSandboxPlan, WarmSandboxState};

async fn prepare(harness: &Harness, candidate: &Sandbox) -> WarmSandbox {
    let mut reaches = candidate.credential.reaches.clone();
    reaches.sort();
    let record = WarmSandbox::new(
        WarmSandboxPlan {
            profile: candidate.profile.clone(),
            workspaces: Vec::new(),
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
    assert!(candidate.work.prepared_installation_digest.is_none());
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
    assert!(active.work.prepared_installation_digest.is_none());
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
    assert!(active.work.prepared_installation_digest.is_none());
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
            workspaces: Vec::new(),
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
