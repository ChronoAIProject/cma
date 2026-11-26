//! A platform spend stop pauses keep-alive and agentless Sandboxes inside the
//! maintenance transaction (decision 26), leaves the owner's preference alone
//! and never pauses busy work.
use super::*;
use layer2_core::budget::{
    BudgetSettings, CloudObservationInput, CloudSpendPolicy, PlatformBudgetPolicy,
    PlatformSpendLedger,
};
use layer2_core::sandbox::store::{InactivityPauseCause, SandboxSessionStore};
use layer2_core::sandbox::PLATFORM_SPEND_PAUSE_NOTICE;

pub(super) async fn install_stop(store: &MongoSandboxStore) {
    let settings = layer2_core::PlatformSettings {
        model_budget: BudgetSettings {
            enabled: true,
            owners: vec![layer2_core::budget::OwnerBudgetPolicy {
                owner: layer2_core::SubjectId::new("unrelated-budget-owner"),
                policy: layer2_core::budget::BudgetPolicy {
                    owner_day_limit: 1000,
                    turn_limit: 100,
                    max_calls_per_turn: 10,
                    day_timezone: "UTC".into(),
                    input_token_units: 1,
                    output_token_units: 1,
                    max_unobserved_units: 1,
                    usage_timeout_seconds: 60,
                },
            }],
            platform: Some(PlatformBudgetPolicy {
                period_cap: 1000,
                period_start: "2026-01-01T00:00:00Z".into(),
                period_end: "2100-01-01T00:00:00Z".into(),
                warning_percent: 80,
                stop_percent: 100,
                protected_reserve: 0,
                lag_allowance: 0,
                cloud: Some(CloudSpendPolicy {
                    run_rate_units_per_day: 1,
                    max_observation_age_seconds: 3600,
                }),
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    settings.model_budget.validate().unwrap();
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {
                "$set": {"settings": mongodb::bson::serialize_to_bson(&settings).unwrap()},
                "$inc": {"revision": 1_i64},
            },
        )
        .await
        .unwrap();
    let now = layer2_core::seconds_from_rfc3339(LATER).unwrap();
    store
        .record_cloud_observation(
            CloudObservationInput {
                observed_through: now,
                units: 1000,
            },
            now,
        )
        .await
        .unwrap();
    store.evaluate_period(now).await.unwrap();
}

fn platform_stop(id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        sandbox_id: id.clone(),
        requested_at: LATER.into(),
        pause_after_seconds: 0,
        auto_pause_enabled: true,
        expected_policy_revision: None,
        cause: InactivityPauseCause::PlatformSpendStop,
    }
}

pub(super) fn workspace(sandbox: &Sandbox, step: WorkspaceWorkStep) -> Workspace {
    Workspace {
        id: WorkspaceId(format!("wks_{}", sandbox.id)),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_platform_stop".into()),
            revision: 1,
            digest: "d".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "stop".into(),
                },
                name: "stop".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "stop".into(),
        path: "/home/cma/stop".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(step, IDLE_AT.into()),
        next_worktree_number: 1,
        created_at: IDLE_AT.into(),
        ready_at: Some(IDLE_AT.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

async fn assert_platform_pause(store: &MongoSandboxStore, before: &Sandbox) {
    assert_eq!(
        store
            .schedule_inactivity_pause(platform_stop(&before.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let paused = store.get_sandbox(&before.id).await.unwrap().unwrap();
    assert_eq!(paused.status, SandboxStatus::Pausing);
    assert_eq!(
        paused.lifecycle.paused_notice.as_deref(),
        Some(PLATFORM_SPEND_PAUSE_NOTICE)
    );
    assert_eq!(
        paused.profile, before.profile,
        "the preference is unchanged"
    );
}

async fn assert_not_paused(store: &MongoSandboxStore, sandbox: &Sandbox) {
    assert_eq!(
        store
            .schedule_inactivity_pause(platform_stop(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible,
        "{}",
        sandbox.id
    );
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Active
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_platform_stop_pauses_a_keep_alive_sandbox_and_keeps_its_preference() {
    let (client, name, store) = database().await;
    install_stop(&store).await;
    let (sandbox, _) = fixture(&store, "sbx_stop_always_on").await;
    let always_on = store
        .set_sandbox_always_on(sandbox.id.clone(), sandbox.revision, true)
        .await
        .unwrap();
    let mut inactivity = pause(&always_on.id);
    inactivity.auto_pause_enabled = true;
    assert_eq!(
        store.schedule_inactivity_pause(inactivity).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_platform_pause(&store, &always_on).await;
    assert_eq!(
        store
            .get_sandbox(&always_on.id)
            .await
            .unwrap()
            .unwrap()
            .profile
            .always_on,
        Some(true)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_platform_stop_pauses_an_agentless_sandbox_unless_a_workspace_or_session_is_busy() {
    let (client, name, store) = database().await;
    install_stop(&store).await;
    let agentless = active_sandbox("sbx_stop_agentless");
    store.create_sandbox(agentless.clone()).await.unwrap();
    assert_eq!(
        store
            .preview_inactivity_pause(pause(&agentless.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_platform_pause(&store, &agentless).await;

    let preparing = active_sandbox("sbx_stop_preparing");
    store.create_sandbox(preparing.clone()).await.unwrap();
    store
        .create_workspace(workspace(&preparing, WorkspaceWorkStep::Prepare))
        .await
        .unwrap();
    assert_not_paused(&store, &preparing).await;

    let watched = active_sandbox("sbx_stop_session");
    store.create_sandbox(watched.clone()).await.unwrap();
    store
        .open_sandbox_session(&watched.id, "session-a", "2026-09-05T01:00:00Z")
        .await
        .unwrap();
    assert_not_paused(&store, &watched).await;

    // Quiet Agents do not make pending Workspace work interruptible either.
    let (setting_up, _) = fixture(&store, "sbx_stop_setup").await;
    store
        .create_workspace(workspace(&setting_up, WorkspaceWorkStep::Setup))
        .await
        .unwrap();
    assert_not_paused(&store, &setting_up).await;
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_stop_pause_request_cannot_pause_without_current_authority() {
    let (client, name, store) = database().await;
    let sandbox = active_sandbox("sbx_stale_stop");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    assert_not_paused(&store, &sandbox).await;
    install_stop(&store).await;
    store
        .budget_settings
        .update_one(
            doc! {"_id": "platform"},
            doc! {
                "$set": {"settings.model_budget.platform.period_cap": 2000_i64},
                "$inc": {"revision": 1_i64},
            },
        )
        .await
        .unwrap();
    store
        .evaluate_period(layer2_core::seconds_from_rfc3339(LATER).unwrap())
        .await
        .unwrap();
    assert_not_paused(&store, &sandbox).await;
    client.database(&name).drop().await.unwrap();
}
