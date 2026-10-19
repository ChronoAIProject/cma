use super::lifecycle_test_support::NOW;
use super::runtime_update_test_support::native;
use super::store::*;
use super::*;

fn paused() -> Sandbox {
    let mut value = native();
    value.begin_pause(NOW.into(), None).unwrap();
    for phase in 0..4 {
        let fence = value
            .work
            .claim("resume-test".into(), NOW, "2026-09-05T01:00:00Z".into())
            .unwrap();
        match phase {
            0 => value.complete_lifecycle_persistent_pause("resume-test", fence, NOW.into()),
            1 => value.complete_lifecycle_agent_pause("resume-test", fence, NOW.into()),
            2 => value.complete_lifecycle_runtime_suspension("resume-test", fence, NOW.into()),
            _ => value.publish_lifecycle_paused("resume-test", fence, NOW.into()),
        }
        .unwrap();
    }
    value
}

#[test]
fn runtime_resume_pins_latest_in_one_revision_and_preserves_original_home() {
    for cause in [
        SandboxReviveCause::ExplicitRequest,
        SandboxReviveCause::AgentMessage,
        SandboxReviveCause::TriggerLaunch,
        SandboxReviveCause::AgentCreation,
        SandboxReviveCause::WorkspaceCreation,
    ] {
        let mut value = paused();
        let before = value.clone();
        value
            .begin_revive_with_managed_runtime(cause, Some("image-v2"), NOW.into())
            .unwrap();
        assert_eq!(value.revision, before.revision + 1);
        assert_eq!(value.status, SandboxStatus::Reviving);
        assert_eq!(value.lifecycle.home, before.lifecycle.home);
        assert_eq!(
            value.lifecycle.suspended_runtime,
            before.lifecycle.suspended_runtime
        );
        assert_eq!(value.profile, before.profile);
        let request = value.runtime_update_request().unwrap();
        assert_eq!(request.target_version, "image-v2");
        assert_eq!(request.started_at.as_deref(), Some(NOW));
        assert!(request.completed_at.is_none());
        let encoded = serde_json::to_vec(&value).unwrap();
        let recovered: Sandbox = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(recovered, value);
    }
}

#[test]
fn runtime_resume_preserves_previously_accepted_target_after_deployment_changes() {
    let mut value = paused();
    value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap();
    let prior = value.runtime_update_request().unwrap().clone();
    let revision = value.revision;
    value
        .begin_revive_with_managed_runtime(
            SandboxReviveCause::ExplicitRequest,
            Some("image-v3"),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(value.revision, revision + 1);
    let request = value.runtime_update_request().unwrap();
    assert_eq!(request.target_version, prior.target_version);
    assert_eq!(request.requested_at, prior.requested_at);
    assert_eq!(request.automatic, prior.automatic);
    assert_eq!(request.started_at.as_deref(), Some(NOW));
}

#[test]
fn runtime_resume_does_not_add_updates_for_loss_custom_images_or_current_versions() {
    for mode in ["loss", "custom", "current", "unconfigured"] {
        let mut value = paused();
        let mut cause = SandboxReviveCause::ExplicitRequest;
        let mut target = Some("image-v2");
        match mode {
            "loss" => cause = SandboxReviveCause::RuntimeLost,
            "custom" => value.profile.content.runtime_image = Some("custom-image".into()),
            "current" => target = Some("image-v1"),
            "unconfigured" => target = None,
            _ => unreachable!(),
        }
        let before = value.clone();
        value
            .begin_revive_with_managed_runtime(cause, target, NOW.into())
            .unwrap();
        assert!(value.runtime_update_request().is_none(), "{mode}");
        assert_eq!(value.revision, before.revision + 1);
        assert_eq!(value.lifecycle.home, before.lifecycle.home);
    }
}

#[test]
fn runtime_resume_creation_uses_same_admission_and_leaves_active_work_unchanged() {
    let mut value = paused();
    let revision = value.revision;
    assert_eq!(
        value
            .record_creation_admission_with_managed_runtime(
                SandboxReviveCause::AgentCreation,
                Some("image-v2"),
                NOW.into()
            )
            .unwrap(),
        Some(SandboxEventKind::StatusChanged {
            from: SandboxStatus::Paused,
            to: SandboxStatus::Reviving
        })
    );
    assert_eq!(value.revision, revision + 1);
    assert_eq!(
        value.runtime_update_request().unwrap().target_version,
        "image-v2"
    );
    let mut active = native();
    let runtime = active.runtime.clone();
    assert!(active
        .record_creation_admission_with_managed_runtime(
            SandboxReviveCause::AgentCreation,
            Some("image-v2"),
            NOW.into()
        )
        .unwrap()
        .is_none());
    assert_eq!(active.runtime, runtime);
    assert!(active.runtime_update_request().is_none());
}

#[tokio::test]
async fn runtime_resume_store_admission_uses_server_target_and_replays_without_retargeting() {
    let store = InMemorySandboxStore::default().with_managed_runtime_version("image-v2".into());
    let mut value = paused();
    value.revision = 1;
    store.create_sandbox(value.clone()).await.unwrap();
    let mut command = SandboxLifecycleCommand::revive(
        value.id.clone(),
        value.owner.clone(),
        NOW.into(),
        SandboxReviveCause::ExplicitRequest,
    );
    command.managed_runtime_version = Some("caller-cannot-select-this".into());
    let SandboxLifecycleCommandResult::Accepted(first) = store
        .schedule_sandbox_lifecycle(command.clone())
        .await
        .unwrap()
    else {
        panic!("resume refused")
    };
    assert_eq!(
        first.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(first.sandbox.revision, 2);
    assert_eq!(
        first
            .sandbox
            .runtime_update_request()
            .unwrap()
            .target_version,
        "image-v2"
    );
    let newer = store.with_managed_runtime_version("image-v3".into());
    let SandboxLifecycleCommandResult::Accepted(replay) =
        newer.schedule_sandbox_lifecycle(command).await.unwrap()
    else {
        panic!("resume replay refused")
    };
    assert_eq!(
        replay.disposition,
        SandboxLifecycleCommandDisposition::AlreadyScheduled
    );
    assert_eq!(replay.sandbox, first.sandbox);
}

#[test]
fn runtime_resume_message_commit_rejects_an_unapproved_target() {
    let current = paused();
    let mut command = SandboxLifecycleCommand::revive(
        current.id.clone(),
        current.owner.clone(),
        NOW.into(),
        SandboxReviveCause::AgentMessage,
    );
    command.managed_runtime_version = Some("image-v2".into());
    let SandboxLifecycleCommandApplication::Commit { sandbox, event, .. } =
        apply_sandbox_lifecycle_command(current.clone(), &command).unwrap()
    else {
        panic!("message resume refused")
    };
    validate_command_revival_transition(&current, &sandbox, NOW, &event, Some("image-v2")).unwrap();
    assert_eq!(
        validate_command_revival_transition(&current, &sandbox, NOW, &event, Some("image-v3")),
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        validate_command_revival_transition(&current, &sandbox, NOW, &event, None),
        Err(SandboxStoreError::Conflict)
    );
}
