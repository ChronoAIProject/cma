use std::sync::Arc;

use crate::ids::{AgentId, SandboxId, SubjectId};
use crate::sandbox::lifecycle_test_support::{agent, log, sandbox, IDLE_AT, NOW};
use crate::sandbox::store::{
    AgentStore, PageRequest, SandboxEventStore, SandboxImageRotationRequest,
    SandboxInactivityPauseRequest, SandboxMaintenanceOutcome, SandboxMaintenanceStore,
    SandboxStore, SandboxStoreError, MAX_PAGE_SIZE,
};
use crate::sandbox::{
    inactivity_pause_notice, Agent, AgentStatus, Sandbox, SandboxEvent, SandboxEventKind,
    SandboxLifecycleOperation, SandboxStatus, SandboxWorkStep,
};

use super::InMemorySandboxStore;

const TARGET_IMAGE: &str = "image-v2";

#[path = "runtime_update_deleted_tests.rs"]
mod deleted;
#[path = "platform_stop_pause_tests.rs"]
mod platform_stop;

#[tokio::test]
async fn keep_alive_policy_applies_to_existing_sandboxes_and_preserves_manual_pause() {
    use crate::profiles::{
        InMemoryProfileStore, ProfileVisibility, SandboxProfile, SandboxProfileStore,
    };
    use crate::sandbox::store::{
        SandboxLifecycleCommand, SandboxLifecycleCommandResult, SandboxLifecycleCommandStore,
    };
    let profiles = InMemoryProfileStore::default();
    let store = InMemorySandboxStore::with_profiles(profiles.clone());
    let value = stored_sandbox("sbx_keepalive", "owner-a");
    let mut profile = SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: value.profile.id.clone(),
        owner: value.owner.clone(),
        name: "Availability".into(),
        description: String::new(),
        visibility: ProfileVisibility::Private,
        forked_from: None,
        uses_count: 0,
        draft: value.profile.content.clone(),
        draft_version: 1,
        created_at: IDLE_AT.into(),
        updated_at: IDLE_AT.into(),
        retired_at: None,
    };
    profiles
        .create_sandbox_profile(profile.clone())
        .await
        .unwrap();
    store.create_sandbox(value.clone()).await.unwrap();
    let first_agent = stored_agent(&value, "agt_keepalive_first", "conversation-first", 1);
    store.create_agent(first_agent.clone()).await.unwrap();
    insert_logs(&store, &first_agent.id, &[IDLE_AT]);
    profile.availability.always_keep_alive = true;
    profile.draft_version = 2;
    profiles
        .save_sandbox_draft(1, profile.clone())
        .await
        .unwrap();
    assert_pause_not_eligible(&store, &value.id).await;

    let enabled = stored_sandbox("sbx_enabled_keepalive", "owner-a");
    store.create_sandbox(enabled.clone()).await.unwrap();
    let enabled_agent = stored_agent(&enabled, "agt_enabled", "conversation-enabled", 1);
    store.create_agent(enabled_agent.clone()).await.unwrap();
    insert_logs(&store, &enabled_agent.id, &[IDLE_AT]);
    let mut enabled_request = pause_request(&enabled.id);
    enabled_request.auto_pause_enabled = true;
    assert_eq!(
        store
            .schedule_inactivity_pause(enabled_request)
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );

    let mut always_on = stored_sandbox("sbx_always_on", "owner-a");
    always_on.profile.always_on = Some(true);
    store.create_sandbox(always_on.clone()).await.unwrap();
    let always_on_agent = stored_agent(&always_on, "agt_always_on", "conversation-always-on", 1);
    store.create_agent(always_on_agent.clone()).await.unwrap();
    insert_logs(&store, &always_on_agent.id, &[IDLE_AT]);
    let mut always_on_request = pause_request(&always_on.id);
    always_on_request.auto_pause_enabled = true;
    assert_eq!(
        store
            .schedule_inactivity_pause(always_on_request)
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );

    assert_eq!(
        store.get_sandbox(&value.id).await.unwrap(),
        Some(value.clone())
    );
    let manual = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            value.id.clone(),
            value.owner.clone(),
            NOW.into(),
            None,
        ))
        .await
        .unwrap();
    assert!(matches!(manual, SandboxLifecycleCommandResult::Accepted(_)));

    let second = stored_sandbox("sbx_policy_disabled", "owner-a");
    store.create_sandbox(second.clone()).await.unwrap();
    let second_agent = stored_agent(&second, "agt_keepalive_second", "conversation-second", 1);
    store.create_agent(second_agent.clone()).await.unwrap();
    insert_logs(&store, &second_agent.id, &[IDLE_AT]);
    assert_pause_not_eligible(&store, &second.id).await;
    profile.availability.always_keep_alive = false;
    profile.draft_version = 3;
    profiles.save_sandbox_draft(2, profile).await.unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(pause_request(&second.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
}

#[tokio::test]
async fn maintenance_candidates_are_global_stable_active_and_bounded() {
    let store = InMemorySandboxStore::default();
    for (id, owner, status) in [
        ("sbx_c", "owner-a", SandboxStatus::Active),
        ("sbx_a", "owner-b", SandboxStatus::Active),
        ("sbx_d", "owner-c", SandboxStatus::Provisioning),
        ("sbx_b", "owner-a", SandboxStatus::Active),
    ] {
        let mut value = stored_sandbox(id, owner);
        value.status = status;
        store.create_sandbox(value).await.unwrap();
    }

    let first = store
        .sandbox_maintenance_candidates(&PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items, ids(&["sbx_a", "sbx_b"]));
    assert_eq!(first.next.as_deref(), Some("sbx_b"));

    let second = store
        .sandbox_maintenance_candidates(&PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items, ids(&["sbx_c"]));
    assert_eq!(second.next, None);

    for invalid in [
        PageRequest {
            limit: 0,
            cursor: None,
        },
        PageRequest {
            limit: MAX_PAGE_SIZE + 1,
            cursor: None,
        },
    ] {
        assert_eq!(
            store.sandbox_maintenance_candidates(&invalid).await,
            Err(SandboxStoreError::InvalidPage)
        );
    }
}

#[tokio::test]
async fn maintenance_discovery_and_scheduling_reject_corrupt_sandbox_identity() {
    let store = InMemorySandboxStore::default();
    let key = SandboxId("sbx_stored_key".into());
    let value = stored_sandbox("sbx_embedded_id", "owner-a");
    store.lock().sandboxes.insert(key.clone(), value);

    assert!(matches!(
        store
            .sandbox_maintenance_candidates(&PageRequest::default())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        store
            .schedule_image_rotation(rotation_request(&key, TARGET_IMAGE))
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(store.sandbox_events(&key, 0, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn inactivity_pause_uses_every_live_agents_exact_newest_log_and_appends_status() {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox("sbx_idle", "owner-a");
    store.create_sandbox(value.clone()).await.unwrap();
    store
        .append_sandbox_event(&value.id, IDLE_AT.into(), SandboxEventKind::Created)
        .await
        .unwrap();

    let first = stored_agent(&value, "agt_z", "conversation-z", 2);
    let second = stored_agent(&value, "agt_a", "conversation-a", 1);
    store.create_agent(first.clone()).await.unwrap();
    store.create_agent(second.clone()).await.unwrap();
    insert_logs(&store, &first.id, &["2026-09-05T00:10:00Z", IDLE_AT]);
    insert_logs(&store, &second.id, &[IDLE_AT]);

    assert_eq!(
        store
            .schedule_inactivity_pause(pause_request(&value.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );

    let scheduled = store.get_sandbox(&value.id).await.unwrap().unwrap();
    assert_eq!(scheduled.status, SandboxStatus::Pausing);
    assert_eq!(scheduled.revision, value.revision + 1);
    assert_eq!(
        scheduled.lifecycle.paused_notice.as_deref(),
        Some(inactivity_pause_notice(900).as_str())
    );
    assert_eq!(scheduled.work.step, SandboxWorkStep::RunLifecycleOperation);
    let Some(SandboxLifecycleOperation::Pause { authority, .. }) =
        scheduled.lifecycle.operation.as_ref()
    else {
        panic!("inactivity did not schedule a pause operation");
    };
    assert_eq!(authority.requested_at, NOW);

    assert_eq!(
        store.sandbox_events(&value.id, 0, 10).await.unwrap(),
        vec![
            SandboxEvent {
                sandbox_id: value.id.clone(),
                seq: 1,
                appended_at: IDLE_AT.into(),
                kind: SandboxEventKind::Created,
            },
            SandboxEvent {
                sandbox_id: value.id.clone(),
                seq: 2,
                appended_at: NOW.into(),
                kind: SandboxEventKind::StatusChanged {
                    from: SandboxStatus::Active,
                    to: SandboxStatus::Pausing,
                },
            },
        ]
    );
}

#[tokio::test]
async fn inactivity_pause_fails_closed_for_missing_stale_recent_and_partial_evidence() {
    let (missing_store, missing_sandbox, _) = idle_fixture("sbx_missing_log");
    missing_store.lock().agent_logs.clear();
    assert_pause_not_eligible(&missing_store, &missing_sandbox.id).await;

    let (partial_store, partial_sandbox, first) = idle_fixture("sbx_partial_set");
    let second = stored_agent(&partial_sandbox, "agt_without_log", "conversation-b", 1);
    partial_store
        .lock()
        .agents
        .insert(second.id.clone(), second);
    assert!(partial_store.lock().agent_logs.contains_key(&first.id));
    assert_pause_not_eligible(&partial_store, &partial_sandbox.id).await;

    let (stale_store, stale_sandbox, stale_agent) = idle_fixture("sbx_stale_seq");
    stale_store
        .lock()
        .agents
        .get_mut(&stale_agent.id)
        .unwrap()
        .conversation
        .last_seq = 2;
    assert_pause_not_eligible(&stale_store, &stale_sandbox.id).await;

    let (recent_store, recent_sandbox, recent_agent) = idle_fixture("sbx_recent_log");
    insert_logs(
        &recent_store,
        &recent_agent.id,
        &[IDLE_AT, "2026-09-05T00:29:59Z"],
    );
    recent_store
        .lock()
        .agents
        .get_mut(&recent_agent.id)
        .unwrap()
        .conversation
        .last_seq = 2;
    assert_pause_not_eligible(&recent_store, &recent_sandbox.id).await;

    let (identity_store, identity_sandbox, identity_agent) = idle_fixture("sbx_partial_identity");
    identity_store
        .lock()
        .agents
        .get_mut(&identity_agent.id)
        .unwrap()
        .conversation
        .runtime_generation = None;
    assert_pause_not_eligible(&identity_store, &identity_sandbox.id).await;
}

#[tokio::test]
async fn corrupt_or_missing_inactivity_authority_never_mutates_or_emits() {
    let (stream_store, stream_sandbox, stream_agent) = idle_fixture("sbx_corrupt_log");
    stream_store
        .lock()
        .agent_logs
        .get_mut(&stream_agent.id)
        .unwrap()[0]
        .seq = 2;
    let before = stream_store.get_sandbox(&stream_sandbox.id).await.unwrap();
    assert_eq!(
        stream_store
            .schedule_inactivity_pause(pause_request(&stream_sandbox.id))
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        stream_store.get_sandbox(&stream_sandbox.id).await.unwrap(),
        before
    );
    assert!(stream_store
        .sandbox_events(&stream_sandbox.id, 0, 10)
        .await
        .unwrap()
        .is_empty());

    let (sandbox_store, corrupt_sandbox, _) = idle_fixture("sbx_corrupt_sandbox");
    sandbox_store
        .lock()
        .sandboxes
        .get_mut(&corrupt_sandbox.id)
        .unwrap()
        .name
        .clear();
    let before = sandbox_store
        .get_sandbox(&corrupt_sandbox.id)
        .await
        .unwrap();
    assert!(matches!(
        sandbox_store
            .schedule_inactivity_pause(pause_request(&corrupt_sandbox.id))
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(
        sandbox_store
            .get_sandbox(&corrupt_sandbox.id)
            .await
            .unwrap(),
        before
    );
    assert!(sandbox_store
        .sandbox_events(&corrupt_sandbox.id, 0, 10)
        .await
        .unwrap()
        .is_empty());

    let absent = SandboxId("sbx_absent".into());
    assert_eq!(
        InMemorySandboxStore::default()
            .schedule_inactivity_pause(pause_request(&absent))
            .await,
        Err(SandboxStoreError::NotFound)
    );
}

#[tokio::test]
async fn image_rotation_captures_all_live_identity_in_order_and_excludes_terminal_agents() {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox("sbx_drift", "owner-a");
    store.create_sandbox(value.clone()).await.unwrap();
    store
        .append_sandbox_event(&value.id, IDLE_AT.into(), SandboxEventKind::Created)
        .await
        .unwrap();

    let later = stored_agent(&value, "agt_z", "conversation-z", 1);
    let earlier = stored_agent(&value, "agt_a", "conversation-a", 1);
    let mut terminal = stored_agent(&value, "agt_terminal", "conversation-terminal", 1);
    terminal.status = AgentStatus::Stopped;
    for entry in [&later, &terminal, &earlier] {
        store.create_agent(entry.clone()).await.unwrap();
    }

    assert_eq!(
        store
            .schedule_image_rotation(rotation_request(&value.id, TARGET_IMAGE))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );

    let rotated = store.get_sandbox(&value.id).await.unwrap().unwrap();
    assert_eq!(rotated.status, SandboxStatus::Rotating);
    assert_eq!(rotated.revision, value.revision + 1);
    assert_eq!(rotated.work.step, SandboxWorkStep::RunLifecycleOperation);
    let Some(SandboxLifecycleOperation::ImageRotation {
        authority,
        target_runtime_version,
        identity,
        ..
    }) = rotated.lifecycle.operation.as_ref()
    else {
        panic!("image drift did not schedule a rotation operation");
    };
    assert_eq!(authority.requested_at, NOW);
    assert_eq!(target_runtime_version, TARGET_IMAGE);
    assert_eq!(identity.sandbox_id, value.id);
    assert_eq!(identity.owner, value.owner);
    assert_eq!(identity.profile_id, value.profile.id);
    assert_eq!(identity.profile_revision, value.profile.revision);
    assert_eq!(identity.profile_digest, value.profile.digest);
    assert_eq!(
        identity
            .agents
            .iter()
            .map(|entry| entry.agent_id.as_str())
            .collect::<Vec<_>>(),
        vec!["agt_a", "agt_z"]
    );
    assert_eq!(identity.agents[0].conversation_id, "conversation-a");
    assert_eq!(identity.agents[1].conversation_id, "conversation-z");

    let events = store.sandbox_events(&value.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[1].appended_at, NOW);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Rotating,
        }
    );
}

#[tokio::test]
async fn image_rotation_replay_is_exact_and_a_different_target_cannot_replace_authority() {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox("sbx_rotation_replay", "owner-a");
    let live = stored_agent(&value, "agt_rotation", "conversation-rotation", 1);
    store.create_sandbox(value.clone()).await.unwrap();
    store.create_agent(live).await.unwrap();
    let request = rotation_request(&value.id, TARGET_IMAGE);

    assert_eq!(
        store
            .schedule_image_rotation(request.clone())
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let scheduled = store.get_sandbox(&value.id).await.unwrap().unwrap();
    let events = store.sandbox_events(&value.id, 0, 10).await.unwrap();

    assert_eq!(
        store.schedule_image_rotation(request).await.unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    assert_eq!(
        store
            .schedule_image_rotation(rotation_request(&value.id, "image-v3"))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&value.id).await.unwrap(), Some(scheduled));
    assert_eq!(
        store.sandbox_events(&value.id, 0, 10).await.unwrap(),
        events
    );
}

#[tokio::test]
async fn incomplete_rotation_identity_and_absent_drift_fail_closed() {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox("sbx_rotation_incomplete", "owner-a");
    let mut incomplete = stored_agent(&value, "agt_incomplete", "conversation-a", 1);
    incomplete.conversation.runtime_generation = None;
    store.create_sandbox(value.clone()).await.unwrap();
    store.create_agent(incomplete).await.unwrap();
    let before = store.get_sandbox(&value.id).await.unwrap();

    assert_eq!(
        store
            .schedule_image_rotation(rotation_request(&value.id, TARGET_IMAGE))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&value.id).await.unwrap(), before);
    assert!(store
        .sandbox_events(&value.id, 0, 10)
        .await
        .unwrap()
        .is_empty());

    let no_drift_store = InMemorySandboxStore::default();
    let mut no_drift = stored_sandbox("sbx_no_drift", "owner-b");
    no_drift.runtime.as_mut().unwrap().runtime_version = TARGET_IMAGE.into();
    no_drift_store
        .create_sandbox(no_drift.clone())
        .await
        .unwrap();
    assert_eq!(
        no_drift_store
            .schedule_image_rotation(rotation_request(&no_drift.id, TARGET_IMAGE))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        no_drift_store.get_sandbox(&no_drift.id).await.unwrap(),
        Some(no_drift)
    );
}

#[tokio::test]
async fn concurrent_identical_maintenance_requests_schedule_each_mutation_once() {
    let (pause_store, pause_sandbox, _) = idle_fixture("sbx_concurrent_pause");
    let pause_store = Arc::new(pause_store);
    let pause_request = pause_request(&pause_sandbox.id);
    let (left, right) = tokio::join!(
        pause_store.schedule_inactivity_pause(pause_request.clone()),
        pause_store.schedule_inactivity_pause(pause_request),
    );
    assert_scheduled_once([left.unwrap(), right.unwrap()]);
    assert_eq!(
        pause_store
            .get_sandbox(&pause_sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        pause_sandbox.revision + 1
    );
    assert_eq!(
        pause_store
            .sandbox_events(&pause_sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );

    let rotation_store = Arc::new(InMemorySandboxStore::default());
    let rotation_sandbox = stored_sandbox("sbx_concurrent_rotation", "owner-a");
    let live = stored_agent(
        &rotation_sandbox,
        "agt_concurrent",
        "conversation-concurrent",
        1,
    );
    rotation_store
        .create_sandbox(rotation_sandbox.clone())
        .await
        .unwrap();
    rotation_store.create_agent(live).await.unwrap();
    let rotation_request = rotation_request(&rotation_sandbox.id, TARGET_IMAGE);
    let (left, right) = tokio::join!(
        rotation_store.schedule_image_rotation(rotation_request.clone()),
        rotation_store.schedule_image_rotation(rotation_request),
    );
    assert_scheduled_once([left.unwrap(), right.unwrap()]);
    assert_eq!(
        rotation_store
            .get_sandbox(&rotation_sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        rotation_sandbox.revision + 1
    );
    assert_eq!(
        rotation_store
            .sandbox_events(&rotation_sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn corrupt_event_sequence_rolls_back_the_aggregate_and_event_append() {
    let (store, value, _) = idle_fixture("sbx_event_gap");
    let corrupt = SandboxEvent {
        sandbox_id: value.id.clone(),
        seq: 2,
        appended_at: IDLE_AT.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .lock()
        .sandbox_events
        .insert(value.id.clone(), vec![corrupt.clone()]);
    let before = store.get_sandbox(&value.id).await.unwrap();

    assert_eq!(
        store
            .schedule_inactivity_pause(pause_request(&value.id))
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_sandbox(&value.id).await.unwrap(), before);
    assert_eq!(
        store.sandbox_events(&value.id, 0, 10).await.unwrap(),
        vec![corrupt]
    );
}

#[tokio::test]
async fn invalid_maintenance_requests_are_rejected_without_mutation() {
    let (store, value, _) = idle_fixture("sbx_invalid_request");
    let before = store.get_sandbox(&value.id).await.unwrap();

    let mut pause = pause_request(&value.id);
    pause.pause_after_seconds = 0;
    assert!(matches!(
        store.schedule_inactivity_pause(pause).await,
        Err(SandboxStoreError::InvalidTransaction(_))
    ));

    let mut rotation = rotation_request(&value.id, TARGET_IMAGE);
    rotation.expected_runtime_version.clear();
    assert!(matches!(
        store.schedule_image_rotation(rotation).await,
        Err(SandboxStoreError::InvalidTransaction(_))
    ));

    assert_eq!(store.get_sandbox(&value.id).await.unwrap(), before);
    assert!(store
        .sandbox_events(&value.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
}

async fn assert_pause_not_eligible(store: &InMemorySandboxStore, sandbox_id: &SandboxId) {
    let before = store.get_sandbox(sandbox_id).await.unwrap();
    let events = store.sandbox_events(sandbox_id, 0, 10).await.unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(pause_request(sandbox_id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(sandbox_id).await.unwrap(), before);
    assert_eq!(
        store.sandbox_events(sandbox_id, 0, 10).await.unwrap(),
        events
    );
}

fn assert_scheduled_once(outcomes: [SandboxMaintenanceOutcome; 2]) {
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == SandboxMaintenanceOutcome::Scheduled)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == SandboxMaintenanceOutcome::AlreadyScheduled)
            .count(),
        1
    );
}

fn idle_fixture(id: &str) -> (InMemorySandboxStore, Sandbox, Agent) {
    let store = InMemorySandboxStore::default();
    let value = stored_sandbox(id, "owner-a");
    let live = stored_agent(&value, "agt_idle", "conversation-idle", 1);
    {
        let mut state = store.lock();
        state.sandboxes.insert(value.id.clone(), value.clone());
        state.agents.insert(live.id.clone(), live.clone());
    }
    insert_logs(&store, &live.id, &[IDLE_AT]);
    (store, value, live)
}

fn stored_sandbox(id: &str, owner: &str) -> Sandbox {
    let mut value = sandbox();
    value.id = SandboxId(id.into());
    value.owner = SubjectId(owner.into());
    value.name = id.into();
    value.revision = 1;
    value
}

fn stored_agent(sandbox: &Sandbox, id: &str, conversation_id: &str, last_seq: u64) -> Agent {
    let mut value = agent(id, conversation_id, last_seq);
    value.sandbox_id = sandbox.id.clone();
    value.owner = sandbox.owner.clone();
    value.revision = 1;
    value
}

fn insert_logs(store: &InMemorySandboxStore, agent_id: &AgentId, timestamps: &[&str]) {
    let entries = timestamps
        .iter()
        .enumerate()
        .map(|(index, appended_at)| {
            log(
                agent_id.as_str(),
                u64::try_from(index).unwrap() + 1,
                appended_at,
            )
        })
        .collect();
    store.lock().agent_logs.insert(agent_id.clone(), entries);
}

fn pause_request(sandbox_id: &SandboxId) -> SandboxInactivityPauseRequest {
    SandboxInactivityPauseRequest {
        sandbox_id: sandbox_id.clone(),
        requested_at: NOW.into(),
        pause_after_seconds: 15 * 60,
        auto_pause_enabled: false,
        cause: Default::default(),
    }
}

fn rotation_request(
    sandbox_id: &SandboxId,
    expected_runtime_version: &str,
) -> SandboxImageRotationRequest {
    SandboxImageRotationRequest {
        sandbox_id: sandbox_id.clone(),
        requested_at: NOW.into(),
        expected_runtime_version: expected_runtime_version.into(),
    }
}

fn ids(values: &[&str]) -> Vec<SandboxId> {
    values
        .iter()
        .map(|value| SandboxId((*value).into()))
        .collect()
}
