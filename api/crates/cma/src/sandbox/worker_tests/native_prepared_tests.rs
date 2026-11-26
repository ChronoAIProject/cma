use super::*;
use crate::sandbox::native_prepared::{prepare_step, retire_step};
use layer2_core::profiles::{content_digest, AgentProfileRef, WorkspaceProfileSnapshot};
use layer2_core::sandbox::{
    ProvisioningExecution, WarmSandbox, WarmSandboxPlan, WarmSandboxState, WarmWorkspacePlan,
};

#[path = "installation_transfer_tests.rs"]
mod installation_transfer_tests;
#[path = "workspace_transfer_tests.rs"]
mod workspace_transfer_tests;

#[path = "native_prepared_guard_tests.rs"]
mod native_prepared_guard_tests;
#[path = "native_prepared_guidance_tests.rs"]
mod native_prepared_guidance_tests;
#[path = "native_prepared_health_tests.rs"]
mod native_prepared_health_tests;

async fn reservation(harness: &Harness) -> WarmSandbox {
    let mut parent = harness.create().await;
    let mut agent = lifecycle_queue_tests::ready_agent(&parent, SystemTime::now()).profile;
    agent.content.skills.clear();
    agent.content.system_prompt.clear();
    agent.digest = content_digest(&agent.content).unwrap();
    let content = WorkspaceProfileContent {
        name: "workspace".into(),
        location: WorkspaceLocation::Directory {
            name: "workspace".into(),
        },
        setup_commands: vec![],
        agent_profiles: vec![AgentProfileRef::new(agent.id.clone(), agent.revision)],
    };
    let workspace = WorkspaceProfileSnapshot {
        id: WorkspaceProfileId::generate(),
        revision: 1,
        digest: content_digest(&content).unwrap(),
        content,
    };
    parent.profile.content.environment_variables.clear();
    parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef {
        id: workspace.id.clone(),
        revision: 1,
    }];
    parent.profile.digest = content_digest(&parent.profile.content).unwrap();
    let mut reaches = parent.credential.reaches.clone();
    reaches.sort();
    let mut record = WarmSandbox::new(
        WarmSandboxPlan {
            profile: parent.profile,
            workspaces: vec![WarmWorkspacePlan {
                profile: workspace,
                agents: vec![agent],
            }],
            reaches,
            runtime_version: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
            execution: ProvisioningExecution::default(),
        },
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    // The native fixture probes this test aggregate before every provider effect.
    // Production authority comes from the separate native-prepared collection.
    record.id = parent.id;
    record
}

fn adapter(harness: &Harness, pools: Arc<Pools>) -> Arc<dyn SandboxAdapter> {
    Arc::new(
        CodexAdapter::with_backend(
            Backend {
                pools: pools.clone(),
                backend: harness.backend.clone(),
            },
            CodexSandboxSpec::default(),
        )
        .with_home_volumes(pools),
    )
}

async fn persist(harness: &Harness, record: WarmSandbox) -> WarmSandbox {
    record.validate().unwrap();
    let record: WarmSandbox =
        serde_json::from_slice(&serde_json::to_vec(&record).unwrap()).unwrap();
    let mut parent = harness.stored(&record.id).await;
    let revision = parent.revision;
    parent.lifecycle.home = record.home.clone();
    parent.revision += 1;
    harness.store.save_sandbox(revision, parent).await.unwrap();
    record
}

async fn advance(
    harness: &Harness,
    pools: &Arc<Pools>,
    record: &WarmSandbox,
) -> layer3_adapter::Result<WarmSandbox> {
    let next = prepare_step(
        adapter(harness, pools.clone()).as_ref(),
        "test",
        record,
        &home_tests::home_policy(),
    )
    .await?;
    Ok(persist(harness, next).await)
}

async fn ready(harness: &Harness, pools: &Arc<Pools>, mut record: WarmSandbox) -> WarmSandbox {
    for _ in 0..20 {
        record = advance(harness, pools, &record).await.unwrap();
        if record.state == WarmSandboxState::Ready {
            return record;
        }
    }
    panic!("prepared hierarchy did not become Ready");
}

#[tokio::test]
async fn native_prepared_restarts_keep_exact_hierarchy_without_key_or_prompt() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let initial = reservation(&harness).await;
    let ids = (
        initial.id.clone(),
        initial.workspaces[0].id.clone(),
        initial.workspaces[0].agents[0].id.clone(),
    );
    let prepared = ready(&harness, &pools, initial).await;
    let verified = advance(&harness, &pools, &prepared).await.unwrap();
    assert_eq!(verified, prepared);
    assert_eq!(
        (
            prepared.id.clone(),
            prepared.workspaces[0].id.clone(),
            prepared.workspaces[0].agents[0].id.clone()
        ),
        ids
    );
    let handle = RuntimeHandle::new(&prepared.runtime.as_ref().unwrap().runtime_id);
    let runtime = harness.backend.snapshot(&handle).await.unwrap();
    assert!(!runtime.credential_delivered);
    assert_eq!(runtime.workspaces.len(), 1);
    assert_eq!(runtime.worktrees.len(), 1);
    assert_eq!(runtime.homes.len(), 1);
    assert_eq!(runtime.agents.len(), 1);
    assert!(prepared.workspaces[0].agents[0].conversation_id.is_some());
    assert!(harness.actions.snapshot().is_empty());
    let home = prepared.home.as_ref().unwrap();
    assert_eq!(home.request.policy.template_name, "remote-seed");
    assert_eq!(
        home.native_pool
            .as_ref()
            .unwrap()
            .claim
            .as_ref()
            .unwrap()
            .uid,
        "claim-uid"
    );
    assert_eq!(home.receipt.as_ref().unwrap().storage_generation, "pvc-uid");
    let calls = pools.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|call| **call == "claim").count(), 1);
    assert_eq!(calls.iter().filter(|call| **call == "adopt").count(), 1);
}

#[tokio::test]
async fn native_workspace_proof_captures_real_receipts_across_preparation_restarts() {
    use crate::sandbox::native_prepared::prepare_step_with_proof;
    use crate::store::native_prepared::NativePreparedWorkspaceProof;
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let mut record = reservation(&harness).await;
    let mut proof: Option<NativePreparedWorkspaceProof> = None;
    for _ in 0..20 {
        (record, proof) = prepare_step_with_proof(
            adapter(&harness, pools.clone()).as_ref(),
            "test",
            &record,
            &home_tests::home_policy(),
            proof,
        )
        .await
        .unwrap();
        record = persist(&harness, record).await;
        proof = serde_json::from_slice(&serde_json::to_vec(&proof).unwrap()).unwrap();
        if let Some(proof) = &proof {
            proof.validate(&record).unwrap();
        }
        if record.state == WarmSandboxState::Ready {
            break;
        }
    }
    assert_eq!(record.state, WarmSandboxState::Ready);
    let proof = proof.unwrap();
    assert!(proof.setup.is_some());
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|(_, frame)| matches!(frame, RequestFrame::PrepareWorkspace(_)))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|(_, frame)| matches!(frame, RequestFrame::WorkspaceSetup(_)))
            .count(),
        1
    );
    let (retained, retained_proof) = prepare_step_with_proof(
        adapter(&harness, pools.clone()).as_ref(),
        "test",
        &record,
        &home_tests::home_policy(),
        Some(proof.clone()),
    )
    .await
    .unwrap();
    assert_eq!(retained, record);
    assert_eq!(retained_proof, Some(proof));
    // A retained legacy Ready row cannot gain fabricated evidence during a health read.
    let (_, absent) = prepare_step_with_proof(
        adapter(&harness, pools).as_ref(),
        "test",
        &record,
        &home_tests::home_policy(),
        None,
    )
    .await
    .unwrap();
    assert!(absent.is_none());
}

#[tokio::test]
async fn native_prepared_invalid_install_receipt_never_publishes_ready() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let mut record = reservation(&harness).await;
    while record.state == WarmSandboxState::Preparing {
        record = advance(&harness, &pools, &record).await.unwrap();
    }
    harness.backend.corrupt_next_install_receipt();
    assert!(advance(&harness, &pools, &record).await.is_err());
    assert!(record.installed_request_sha256.is_none());
    assert!(record.workspaces[0].resolved_commit.is_none());
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn native_prepared_supervisor_replacement_retires_original_authority() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let prepared = ready(&harness, &pools, reservation(&harness).await).await;
    let handle = RuntimeHandle::new(&prepared.runtime.as_ref().unwrap().runtime_id);
    harness
        .backend
        .restart_persistent_process(&handle)
        .await
        .unwrap();
    let retired = advance(&harness, &pools, &prepared).await.unwrap();
    assert_eq!(retired.state, WarmSandboxState::Retiring);
    assert_eq!(retired.runtime, prepared.runtime);
    assert_eq!(retired.home, prepared.home);
    assert_eq!(retired.workspaces, prepared.workspaces);
}

#[tokio::test]
async fn native_prepared_owner_credential_contamination_retires_exact_runtime() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let prepared = ready(&harness, &pools, reservation(&harness).await).await;
    let adapter = adapter(&harness, pools.clone());
    prepare_step(
        adapter.as_ref(),
        "test",
        &prepared,
        &home_tests::home_policy(),
    )
    .await
    .unwrap();
    let handle = RuntimeHandle::new(&prepared.runtime.as_ref().unwrap().runtime_id);
    adapter
        .deliver_credential(
            &handle,
            layer3_adapter::CredentialFrame {
                operation_id: "fixture-contamination".into(),
                value: layer3_adapter::RuntimeCredential::new("opaque-native-fixture"),
                custody_generation: Some(1),
                github_route: None,
            },
        )
        .await
        .unwrap();
    let retired = advance(&harness, &pools, &prepared).await.unwrap();
    assert_eq!(retired.state, WarmSandboxState::Retiring);
    assert_eq!(retired.runtime, prepared.runtime);
    assert_eq!(retired.home, prepared.home);
    assert_eq!(retired.workspaces, prepared.workspaces);
}

#[tokio::test]
async fn native_prepared_lost_provider_replies_reuse_original_claim_and_disk() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &["claim", "observe", "adopt"]);
    let mut record = reservation(&harness).await;
    let id = record.id.clone();
    let mut failures = 0;
    for _ in 0..24 {
        match advance(&harness, &pools, &record).await {
            Ok(next) => record = next,
            Err(AdapterError::Unavailable(_)) => failures += 1,
            Err(error) => panic!("unexpected provider failure: {error}"),
        }
        if record.state == WarmSandboxState::Ready {
            break;
        }
    }
    assert_eq!(failures, 3);
    assert_eq!(record.state, WarmSandboxState::Ready);
    assert_eq!(record.id, id);
    let home = record.home.as_ref().unwrap();
    assert_eq!(
        home.native_pool
            .as_ref()
            .unwrap()
            .claim
            .as_ref()
            .unwrap()
            .uid,
        "claim-uid"
    );
    assert_eq!(home.receipt.as_ref().unwrap().storage_generation, "pvc-uid");
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn native_prepared_cleanup_persists_claim_member_proof_before_home_release() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let mut record = reservation(&harness).await;
    while record
        .home
        .as_ref()
        .and_then(|home| home.attachment())
        .is_none()
    {
        record = advance(&harness, &pools, &record).await.unwrap();
    }
    assert!(record.runtime.is_none());
    record.state = WarmSandboxState::Retiring;
    record = persist(&harness, record).await;
    for _ in 0..10 {
        record = persist(
            &harness,
            retire_step(adapter(&harness, pools.clone()).as_ref(), &record)
                .await
                .unwrap(),
        )
        .await;
        if record.state == WarmSandboxState::Retired {
            break;
        }
    }
    assert_eq!(record.state, WarmSandboxState::Retired);
    assert!(record.home.as_ref().unwrap().released);
    assert!(pools.claim.lock().unwrap().is_none());
    assert!(pools.disk.lock().unwrap().is_none());
    assert_eq!(
        retire_step(adapter(&harness, pools.clone()).as_ref(), &record)
            .await
            .unwrap(),
        record
    );
    let calls = pools.calls.lock().unwrap();
    assert!(
        calls.iter().position(|call| *call == "cleanup").unwrap()
            < calls.iter().position(|call| *call == "remove").unwrap()
    );
}

#[tokio::test]
async fn native_prepared_rejects_custom_setup_before_allocating_capacity() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let mut record = reservation(&harness).await;
    record.plan.workspaces[0]
        .profile
        .content
        .setup_commands
        .push("echo user work".into());
    record.plan.workspaces[0].profile.digest =
        content_digest(&record.plan.workspaces[0].profile.content).unwrap();
    assert!(advance(&harness, &pools, &record).await.is_err());
    assert!(pools.calls.lock().unwrap().is_empty());
    assert!(harness.actions.snapshot().is_empty());
}
