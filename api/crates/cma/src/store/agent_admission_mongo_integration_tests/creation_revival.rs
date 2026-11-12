use super::*;
use layer2_core::sandbox::store::{
    SandboxEventStore, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionStore,
};
use layer2_core::sandbox::{
    SandboxCheckpointRef, SandboxEventKind, SandboxLifecycleOperation, SandboxReviveCause,
};
use layer3_adapter::{CheckpointReceipt, WriterState};

async fn pause(store: &MongoSandboxStore, mut sandbox: Sandbox) -> Sandbox {
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Paused;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    sandbox.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/paused-create.tar.zst".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &[],
            NOW,
        )
        .unwrap(),
    );
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox.clone()).await.unwrap();
    sandbox
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn paused_agent_admission_replays_one_revival_after_store_reopen() {
    let (client, database, store, sandbox, workspace, request) = fixture("paused_replay").await;
    let paused = pause(&store, sandbox).await;
    let (left, right) = tokio::join!(
        store.admit_agent(request.clone()),
        store.admit_agent(request.clone())
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, AgentAdmissionResult::Created(_)))
            .count(),
        1
    );
    assert_eq!(results[0].creation(), results[1].creation());
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    let reviving = reopened.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::AgentCreation,
            ..
        })
    ));
    assert_eq!(
        reopened.admit_agent(request).await.unwrap().creation(),
        results[0].creation()
    );
    assert_eq!(
        reopened.get_sandbox(&paused.id).await.unwrap(),
        Some(reviving.clone())
    );
    reopened
        .admit_agent(admission(&workspace, "during_revival"))
        .await
        .unwrap();
    assert_eq!(
        reopened.get_sandbox(&paused.id).await.unwrap(),
        Some(reviving)
    );
    assert_eq!(
        reopened
            .sandbox_events(&paused.id, 0, 20)
            .await
            .unwrap()
            .iter()
            .filter(|event| matches!(
                event.kind,
                SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Paused,
                    to: SandboxStatus::Reviving
                }
            ))
            .count(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn paused_agent_quota_and_owner_refusals_do_not_revive() {
    let (client, database, store, mut sandbox, _, request) = fixture("paused_refusal").await;
    let sandbox_id = sandbox.id.clone();
    let revision = sandbox.revision;
    sandbox.limits.max_agents_per_workspace = 1;
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox).await.unwrap();
    store.admit_agent(request.clone()).await.unwrap();
    let parent = store.get_sandbox(&sandbox_id).await.unwrap().unwrap();
    let paused = pause(&store, parent).await;
    let mut refused = request;
    refused.creation_key = "over-limit".into();
    refused.agent_id = AgentId::generate();
    refused.worktree_id = WorktreeId::generate();
    assert!(matches!(
        store.admit_agent(refused.clone()).await,
        Err(SandboxStoreError::AgentLimitReached { .. })
    ));
    refused.owner = SubjectId::new("different-owner");
    assert_eq!(
        store.admit_agent(refused).await.unwrap_err(),
        SandboxStoreError::WorkspaceNotReady
    );
    assert_eq!(store.get_sandbox(&paused.id).await.unwrap(), Some(paused));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn paused_workspace_name_conflict_does_not_revive_and_create_is_durable() {
    let (client, database, store, sandbox, workspace, _) = fixture("paused_workspace").await;
    let paused = pause(&store, sandbox).await;
    let mut candidate = workspace.clone();
    candidate.id = WorkspaceId::generate();
    candidate.status = WorkspaceStatus::Preparing;
    candidate.resolved_commit = None;
    candidate.ready_at = None;
    candidate.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let mut request = WorkspaceAdmission {
        binding: None,
        workspace: candidate,
        kind: WorkspaceAdmissionKind::Manual,
        event_at: NOW.into(),
    };
    assert_eq!(
        store.admit_workspace(request.clone()).await.unwrap_err(),
        SandboxStoreError::WorkspaceNameConflict
    );
    assert_eq!(
        store.get_sandbox(&paused.id).await.unwrap(),
        Some(paused.clone())
    );
    request.workspace.name = "new-project".into();
    request.workspace.path = "/home/cma/new-project".into();
    store.admit_workspace(request.clone()).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.get_workspace(&request.workspace.id).await.unwrap(),
        Some(request.workspace)
    );
    let reviving = reopened.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::WorkspaceCreation,
            ..
        })
    ));
    assert_eq!(
        reopened
            .sandbox_events(&paused.id, 0, 20)
            .await
            .unwrap()
            .len(),
        2
    );
    client.database(&database).drop().await.unwrap();
}

/// Enabled budgets and an observed open period row carrying `closures`.
async fn platform_closures(
    client: &Client,
    database: &str,
    closures: &[layer2_core::budget::ClosureReason],
) {
    use layer2_core::budget::{
        BudgetSettings, Closure, CloudObservation, CloudSpendPolicy, PlatformBudgetPolicy,
        PlatformPeriodLedger,
    };
    use layer2_core::PlatformSettingsStore;
    let platform = PlatformBudgetPolicy {
        period_cap: u64::from(u32::MAX),
        period_start: "2026-01-01T00:00:00Z".into(),
        period_end: "2100-01-01T00:00:00Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 0,
        lag_allowance: 0,
        cloud: Some(CloudSpendPolicy {
            run_rate_units_per_day: 1,
            max_observation_age_seconds: (1 << 53) - 1,
        }),
    };
    let settings_store = crate::store::MongoPlatformSettingsStore::open(client, database)
        .await
        .unwrap();
    let settings = layer2_core::PlatformSettings {
        model_budget: BudgetSettings {
            enabled: true,
            platform: Some(platform.clone()),
            // Enabled budgets need a policy; Agent reuse reserves no liability.
            default_policy: Some(layer2_core::budget::BudgetPolicy {
                owner_day_limit: 255,
                turn_limit: 255,
                max_calls_per_turn: 255,
                day_timezone: "UTC".into(),
                input_token_units: 1,
                output_token_units: 1,
                max_unobserved_units: 1,
                usage_timeout_seconds: 60,
            }),
            owners: vec![],
            request_models: vec![],
        },
        ..Default::default()
    };
    let revision = settings_store
        .read()
        .await
        .map_or(0, |stored| stored.revision);
    settings_store.replace(revision, &settings).await.unwrap();
    let mut ledger = PlatformPeriodLedger::open(&platform, 1).unwrap();
    ledger
        .record_cloud(CloudObservation {
            observed_through: ledger.period_start,
            units: 0,
            recorded_at: ledger.period_start,
            settings_revision: 1,
        })
        .unwrap();
    ledger.closures = closures
        .iter()
        .map(|reason| Closure {
            reason: *reason,
            since: ledger.period_start,
            settings_revision: 1,
            committed_at_entry: 0,
        })
        .collect();
    let key = crate::store::budget::period_key(ledger.period_start);
    client
        .database(database)
        .collection::<Document>("model_budget_reservations")
        .replace_one(
            doc! {"_id": &key},
            doc! {"_id": &key,
            "document_schema_version": crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            "record": mongodb::bson::serialize_to_bson(&ledger).unwrap()},
        )
        .upsert(true)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_profile_reuse_does_not_revive_a_paused_sandbox_under_a_platform_stop() {
    use layer2_core::budget::{BudgetError, ClosureReason};
    let (client, database, store, parent, mut workspace, mut automatic) =
        fixture("paused_reuse_stop").await;
    workspace.profile.content.agent_profiles = vec![layer2_core::profiles::AgentProfileRef::new(
        automatic.profile.id.clone(),
        automatic.profile.revision,
    )];
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    let revision = workspace.revision;
    workspace.revision += 1;
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    automatic.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    let mut trigger = automatic.clone();
    trigger.agent_id = AgentId::generate();
    trigger.worktree_id = WorktreeId::generate();
    trigger.origin = AgentOrigin::Trigger {
        trigger_id: "trg_paused_reuse".into(),
        launch_id: "lch_paused_reuse".into(),
    };
    trigger.creation_key = format!(
        "trigger-profile:{}",
        layer2_core::profiles::content_digest(&trigger.origin).unwrap()
    );
    store.admit_agent(automatic).await.unwrap();
    let current = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let paused = pause(&store, current).await;
    for (closure, refusal) in [
        (ClosureReason::SpendStop, BudgetError::PlatformExhausted),
        (ClosureReason::MeteringUnavailable, BudgetError::Unavailable),
    ] {
        platform_closures(&client, &database, &[closure]).await;
        assert_eq!(
            store
                .admit_agent_reusing_profile(trigger.clone())
                .await
                .unwrap_err(),
            SandboxStoreError::Budget(refusal)
        );
        assert_eq!(
            store.get_sandbox(&paused.id).await.unwrap(),
            Some(paused.clone()),
            "the refused reuse did not revive the Sandbox"
        );
        assert_eq!(
            store
                .agent_reuse_bindings
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    platform_closures(&client, &database, &[]).await;
    store.admit_agent_reusing_profile(trigger).await.unwrap();
    let reviving = store.get_sandbox(&paused.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::TriggerLaunch,
            ..
        })
    ));
    client.database(&database).drop().await.unwrap();
}
