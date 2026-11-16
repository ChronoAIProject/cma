//! A platform spend stop pauses keep-alive and agentless Sandboxes too
//! (decision 26), never busy ones, and never changes the owner's preference.
use super::*;
use crate::ids::WorkspaceProfileId;
use crate::profiles::{
    InMemoryProfileStore, ProfileVisibility, SandboxProfile, SandboxProfileStore,
    WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use crate::sandbox::store::{InactivityPauseCause, SandboxSessionStore};
use crate::sandbox::{
    Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
    WorkspaceWorkStep, PLATFORM_SPEND_PAUSE_NOTICE,
};

fn platform_stop(sandbox_id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        sandbox_id: sandbox_id.clone(),
        requested_at: NOW.into(),
        pause_after_seconds: 0,
        auto_pause_enabled: true,
        cause: InactivityPauseCause::PlatformSpendStop,
    }
}

fn workspace(sandbox: &Sandbox, step: WorkspaceWorkStep) -> Workspace {
    Workspace {
        id: crate::ids::WorkspaceId(format!("wks_{}", sandbox.id.as_str())),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_platform_stop".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(step, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

async fn idle_sandbox(store: &InMemorySandboxStore, id: &str) -> Sandbox {
    let value = stored_sandbox(id, "owner-a");
    store.create_sandbox(value.clone()).await.unwrap();
    let agent = stored_agent(
        &value,
        &format!("agt_{id}"),
        &format!("conversation-{id}"),
        1,
    );
    store.create_agent(agent.clone()).await.unwrap();
    insert_logs(store, &agent.id, &[IDLE_AT]);
    value
}

async fn assert_platform_pause(store: &InMemorySandboxStore, before: &Sandbox) {
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
    assert_eq!(paused.profile.always_on, before.profile.always_on);
}

#[tokio::test]
async fn a_platform_stop_pauses_keep_alive_sandboxes_without_changing_the_preference() {
    let profiles = InMemoryProfileStore::default();
    let store = InMemorySandboxStore::with_profiles(profiles.clone());
    let mut always_on = stored_sandbox("sbx_stop_always_on", "owner-a");
    always_on.profile.always_on = Some(true);
    store.create_sandbox(always_on.clone()).await.unwrap();
    let agent = stored_agent(&always_on, "agt_stop_always_on", "conversation-on", 1);
    store.create_agent(agent.clone()).await.unwrap();
    insert_logs(&store, &agent.id, &[IDLE_AT]);
    let mut inactivity = pause_request(&always_on.id);
    inactivity.auto_pause_enabled = true;
    for preview in [false, true] {
        let outcome = if preview {
            store.preview_inactivity_pause(inactivity.clone()).await
        } else {
            store.schedule_inactivity_pause(inactivity.clone()).await
        };
        assert_eq!(outcome.unwrap(), SandboxMaintenanceOutcome::NotEligible);
    }
    assert_eq!(
        store
            .preview_inactivity_pause(platform_stop(&always_on.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_platform_pause(&store, &always_on).await;

    // Historical profile availability keeps a Sandbox alive only for
    // inactivity; the platform stop pauses it and leaves the profile alone.
    let historical = idle_sandbox(&store, "sbx_stop_historical").await;
    let mut profile = SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: historical.profile.id.clone(),
        owner: historical.owner.clone(),
        name: "Availability".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: historical.profile.content.clone(),
        draft_version: 1,
        created_at: IDLE_AT.into(),
        updated_at: IDLE_AT.into(),
        retired_at: None,
    };
    profile.availability.always_keep_alive = true;
    profiles
        .create_sandbox_profile(profile.clone())
        .await
        .unwrap();
    assert_pause_not_eligible(&store, &historical.id).await;
    let mut stop = platform_stop(&historical.id);
    stop.auto_pause_enabled = false;
    assert_eq!(
        store.schedule_inactivity_pause(stop).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    assert_eq!(
        profiles
            .sandbox_profile(&profile.id)
            .await
            .unwrap()
            .unwrap()
            .availability,
        profile.availability
    );
}

#[tokio::test]
async fn a_platform_stop_pauses_agentless_sandboxes_unless_a_workspace_or_session_is_busy() {
    let store = InMemorySandboxStore::default();
    let agentless = stored_sandbox("sbx_stop_agentless", "owner-a");
    store.create_sandbox(agentless.clone()).await.unwrap();
    let mut inactivity = pause_request(&agentless.id);
    inactivity.auto_pause_enabled = true;
    assert_eq!(
        store.schedule_inactivity_pause(inactivity).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_platform_pause(&store, &agentless).await;

    let preparing = stored_sandbox("sbx_stop_preparing", "owner-a");
    store.create_sandbox(preparing.clone()).await.unwrap();
    let pending = workspace(&preparing, WorkspaceWorkStep::Prepare);
    store.lock().workspaces.insert(pending.id.clone(), pending);
    let watched = stored_sandbox("sbx_stop_session", "owner-a");
    store.create_sandbox(watched.clone()).await.unwrap();
    store
        .open_sandbox_session(&watched.id, "session-a", "2026-09-05T01:00:00Z")
        .await
        .unwrap();
    for busy in [&preparing, &watched] {
        assert_eq!(
            store
                .schedule_inactivity_pause(platform_stop(&busy.id))
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::NotEligible,
            "{}",
            busy.id.as_str()
        );
        assert_eq!(
            store.get_sandbox(&busy.id).await.unwrap().unwrap().status,
            SandboxStatus::Active
        );
    }
}

#[tokio::test]
async fn a_platform_stop_never_pauses_a_sandbox_with_busy_agents() {
    let store = InMemorySandboxStore::default();
    let value = idle_sandbox(&store, "sbx_stop_busy").await;
    let mut agent = store
        .get_agent(&AgentId("agt_sbx_stop_busy".into()))
        .await
        .unwrap()
        .unwrap();
    agent.conversation.active_turn = Some(1);
    store.lock().agents.insert(agent.id.clone(), agent);
    assert_eq!(
        store
            .schedule_inactivity_pause(platform_stop(&value.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    let idle_workspace = idle_sandbox(&store, "sbx_stop_workspace").await;
    let pending = workspace(&idle_workspace, WorkspaceWorkStep::Setup);
    store.lock().workspaces.insert(pending.id.clone(), pending);
    assert_eq!(
        store
            .schedule_inactivity_pause(platform_stop(&idle_workspace.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
}
