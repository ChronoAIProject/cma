use super::*;
use crate::sandbox::warmup::prepare_environment;
use layer2_core::sandbox::{ProvisioningExecution, WarmSandbox, WarmSandboxPlan, WarmSandboxState};

#[tokio::test]
async fn full_warm_environment_has_workspace_worktree_home_and_idle_session_before_credentials() {
    use crate::sandbox::agent_skills::EmptyAgentSkillResolver;
    use crate::sandbox::warmup::prepare_hierarchy_step;
    use layer2_core::profiles::{
        content_digest, AgentProfileRef, WorkspaceLocation, WorkspaceProfileContent,
        WorkspaceProfileRef, WorkspaceProfileSnapshot,
    };
    use layer2_core::sandbox::WarmWorkspacePlan;
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let mut candidate = harness.create().await;
    let mut agent = lifecycle_queue_tests::ready_agent(&candidate, SystemTime::now()).profile;
    agent.digest = content_digest(&agent.content).unwrap();
    let content = WorkspaceProfileContent {
        name: "warm-research".into(),
        location: WorkspaceLocation::Directory {
            name: "warm-research".into(),
        },
        setup_commands: vec![
            "prepare public dependencies".into(),
            "# cma:after-credentials\nconfigure owner identity".into(),
        ],
        agent_profiles: vec![AgentProfileRef {
            id: agent.id.clone(),
            revision: agent.revision,
        }],
    };
    let workspace = WorkspaceProfileSnapshot {
        id: layer2_core::WorkspaceProfileId::generate(),
        revision: 1,
        digest: content_digest(&content).unwrap(),
        content,
    };
    candidate.profile.content.workspace_profiles = vec![WorkspaceProfileRef {
        id: workspace.id.clone(),
        revision: 1,
    }];
    candidate.profile.digest = content_digest(&candidate.profile.content).unwrap();
    let mut reaches = candidate.credential.reaches.clone();
    reaches.sort();
    let mut record = WarmSandbox::new(
        WarmSandboxPlan {
            profile: candidate.profile.clone(),
            workspaces: vec![WarmWorkspacePlan {
                profile: workspace,
                agents: vec![agent],
            }],
            reaches,
            runtime_version: runtime_image(),
            execution: ProvisioningExecution::default(),
        },
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    let identities = (
        record.workspaces[0].id.clone(),
        record.workspaces[0].agents[0].id.clone(),
    );
    for _ in 0..10 {
        record = prepare_environment(harness.adapter.as_ref(), "test", &record, None)
            .await
            .unwrap();
        if record.state == WarmSandboxState::Installing && record.installed_request_sha256.is_some()
        {
            prepare_hierarchy_step(
                harness.adapter.as_ref(),
                &EmptyAgentSkillResolver,
                &candidate.owner,
                &mut record,
            )
            .await
            .unwrap();
        }
        record.validate().unwrap();
        // Exercise the exact durable serialization used after every worker step.
        record = serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
        if record.state == WarmSandboxState::Ready {
            break;
        }
    }
    assert_eq!(record.state, WarmSandboxState::Ready);
    assert_eq!(
        (&record.workspaces[0].id, &record.workspaces[0].agents[0].id),
        (&identities.0, &identities.1)
    );
    let handle = RuntimeHandle::new(&record.runtime.as_ref().unwrap().runtime_id);
    let prepared = harness.backend.snapshot(&handle).await.unwrap();
    assert!(!prepared.credential_delivered);
    assert_eq!(prepared.workspaces.len(), 1);
    assert_eq!(prepared.worktrees.len(), 1);
    assert_eq!(prepared.homes.len(), 1);
    assert_eq!(prepared.agents.len(), 1);
    assert!(record.workspaces[0].agents[0].conversation_id.is_some());
    assert!(harness.actions.snapshot().is_empty());
    let requests = harness.backend.requests().await;
    let setup = requests
        .iter()
        .find_map(|(_, request)| match request {
            RequestFrame::WorkspaceSetup(request) => Some(request),
            _ => None,
        })
        .unwrap();
    assert_eq!(setup.commands, ["prepare public dependencies"]);
    let mut incomplete = record.clone();
    incomplete.workspaces[0].agents[0].conversation_id = None;
    assert!(incomplete
        .claim(
            &mut candidate.clone(),
            &runtime_image(),
            &layer2_core::now_rfc3339()
        )
        .is_err());
    record
        .claim(
            &mut candidate,
            &runtime_image(),
            &layer2_core::now_rfc3339(),
        )
        .unwrap();
    assert_eq!(candidate.id, record.id);
    assert_eq!(
        candidate.runtime.as_ref().unwrap().runtime_id,
        handle.as_str()
    );
    assert_eq!(record.state, WarmSandboxState::Claimed);
    assert!(record
        .claim(
            &mut candidate.clone(),
            &runtime_image(),
            &layer2_core::now_rfc3339()
        )
        .is_err());
}

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
        request: &layer3_adapter::HomeVolumeRequest,
        receipt: Option<&layer3_adapter::HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        assert_eq!(request, &self.0.request);
        assert_eq!(receipt, Some(&self.0.receipt));
        Ok(())
    }
}

async fn persistent_warm(harness: &Harness) -> (Arc<dyn SandboxAdapter>, WarmSandbox) {
    let candidate = harness.create().await;
    let mut reaches = candidate.credential.reaches.clone();
    reaches.sort();
    let mut record = WarmSandbox::new(
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
async fn persistent_warm_retirement_releases_capacity_with_historical_boot_identity() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (adapter, mut record) = persistent_warm(&harness).await;
    let original_runtime = record.runtime.clone();
    assert!(original_runtime.as_ref().unwrap().boot_id.is_some());
    record.state = WarmSandboxState::Retiring;
    record.validate().unwrap();

    let retired = prepare_environment(adapter.as_ref(), "test", &record, None)
        .await
        .unwrap();
    assert_eq!(retired.state, WarmSandboxState::Retired);
    assert!(adapter.runtime_inventory().await.unwrap().is_empty());
    assert!(retired.home.as_ref().unwrap().released);
    assert!(retired.home.as_ref().unwrap().attachment().is_none());
    assert_eq!(retired.runtime, original_runtime);
    retired.validate().unwrap();
    let restored: WarmSandbox =
        serde_json::from_slice(&serde_json::to_vec(&retired).unwrap()).unwrap();
    restored.validate().unwrap();
    assert!(!restored.state.owns_runtime());
    assert!(!restored.state.counts_toward_target());

    // Repeated cleanup remains valid, but a released home can never be
    // republished or claimed as a usable runtime.
    let repeated = prepare_environment(adapter.as_ref(), "test", &restored, None)
        .await
        .unwrap();
    assert_eq!(repeated, restored);
    for state in [
        WarmSandboxState::Preparing,
        WarmSandboxState::Installing,
        WarmSandboxState::Ready,
        WarmSandboxState::Claimed,
        WarmSandboxState::Retiring,
    ] {
        let mut invalid = restored.clone();
        invalid.state = state;
        assert!(invalid.validate().is_err(), "{state:?}");
    }
}

#[tokio::test]
async fn legacy_cleanup_refuses_preparation_and_claimed_runtime_ownership() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (adapter, record) = persistent_warm(&harness).await;
    let before = adapter.runtime_inventory().await.unwrap();
    for state in [
        WarmSandboxState::Preparing,
        WarmSandboxState::Installing,
        WarmSandboxState::Ready,
        WarmSandboxState::Claimed,
    ] {
        let mut invalid = record.clone();
        invalid.state = state;
        assert!(
            crate::sandbox::warmup::retire_environment(adapter.as_ref(), &invalid)
                .await
                .is_err()
        );
        assert_eq!(adapter.runtime_inventory().await.unwrap(), before);
        assert!(!invalid.home.as_ref().unwrap().released);
    }
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
