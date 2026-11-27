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
fn retained_startup_policy_keeps_the_admitted_image_on_ordinary_resume() {
    use layer3_adapter::{RuntimeStartupPolicyObservation, RuntimeStatus};

    let current = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let released = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    for enabled in [false, true] {
        let mut sandbox = paused();
        let binding = sandbox.lifecycle.suspended_runtime.as_mut().unwrap();
        binding.runtime_version = current.clone();
        let observation = RuntimeStartupPolicyObservation {
            runtime: RuntimeStatus {
                agent_browser: false,
                agent_filesystem: false,
                agent_attachment_upload: false,
                agent_skill_install: false,
                agent_skill_upgrade: false,
                agent_model_catalog: false,
                agent_resource_preparation: false,
                agent_process_restart: false,
                exact_github_credential_route: false,
                github_repository_scope: false,
                sandbox_id: sandbox.id.to_string(),
                generation: binding.provision_generation.unwrap(),
                home_identity: sandbox
                    .lifecycle
                    .home
                    .as_ref()
                    .unwrap()
                    .attachment()
                    .unwrap()
                    .identity()
                    .unwrap(),
                boot_id: binding.boot_id.clone().unwrap(),
                installation_ready: true,
                credential_ready: true,
                restore_pending: false,
                agent_processes: 0,
            },
            legacy_journal_recovery_enabled: enabled,
        };
        sandbox.lifecycle.retained_startup_policy =
            Some(Box::new(RetainedRuntimePolicyBinding { observation }));
        sandbox.validate().unwrap();
        let original = sandbox.clone();
        assert!(sandbox.supports_retained_runtime_update());
        assert!(!sandbox.check_daily_runtime_update(&released, NOW).unwrap());
        assert!(sandbox
            .request_runtime_update(released.clone(), false, NOW.into())
            .is_err());
        assert_eq!(sandbox, original);
        for cause in [
            SandboxReviveCause::ExplicitRequest,
            SandboxReviveCause::AgentMessage,
            SandboxReviveCause::TriggerLaunch,
            SandboxReviveCause::AgentCreation,
            SandboxReviveCause::WorkspaceCreation,
        ] {
            let mut resumed = original.clone();
            resumed
                .begin_revive_with_managed_runtime(cause, Some(&released), NOW.into())
                .unwrap();
            assert_eq!(resumed.status, SandboxStatus::Reviving);
            assert_eq!(resumed.revision, original.revision + 1);
            assert!(resumed.admitted_runtime_update_target().is_none());
            assert_eq!(
                resumed.lifecycle.suspended_runtime,
                original.lifecycle.suspended_runtime
            );
            assert_eq!(
                resumed.lifecycle.retained_startup_policy,
                original.lifecycle.retained_startup_policy
            );
            assert_eq!(resumed.lifecycle.home, original.lifecycle.home);
        }
    }
}

#[test]
fn cold_canary_keeps_its_exact_runtime_across_daily_updates_and_revive() {
    use super::custom_runtime::test_support::{finish, image, pause, provision};

    for (digit, desktop) in [('a', "0"), ('b', "1")] {
        let mut sandbox = native();
        let pinned = image(digit);
        sandbox.origin = SandboxOrigin::Lifecycle {
            operation_id: format!("canary-fixture-{digit}"),
        };
        sandbox.profile.revision = 1;
        sandbox.profile.content.runtime_image = Some(pinned.clone());
        sandbox.profile.digest = crate::profiles::content_digest(&sandbox.profile.content).unwrap();
        sandbox.runtime.as_mut().unwrap().runtime_version = pinned.clone();

        let home = sandbox.lifecycle.home.as_mut().unwrap();
        let native_pool = home.native_pool.as_mut().unwrap();
        native_pool.request.pool.spec.cold_canary = true;
        native_pool.request.pool.spec.image.uri = pinned.clone();
        native_pool
            .request
            .pool
            .spec
            .image
            .labels
            .insert("dev.chrono.cma.runtime-desktop".into(), desktop.into());
        native_pool.claim.as_mut().unwrap().request = native_pool.request.clone();
        home.receipt
            .as_mut()
            .unwrap()
            .native_authority
            .as_mut()
            .unwrap()
            .claim = native_pool.claim.clone().unwrap();
        sandbox.validate().unwrap();
        assert!(!sandbox.has_managed_runtime_image());

        let original = sandbox.clone();
        for (target, at) in [
            (image('c'), "2026-10-19T06:00:00Z"),
            (pinned.clone(), "2026-10-19T18:00:00Z"),
            (pinned.clone(), "2026-10-20T18:00:00Z"),
            (image('d'), "2026-10-21T18:00:00Z"),
            (image('d'), "2026-10-22T18:00:00Z"),
        ] {
            assert!(!sandbox.check_daily_runtime_update(&target, at).unwrap());
            assert_eq!(sandbox, original);
        }

        pause(&mut sandbox);
        let paused = sandbox.clone();
        let newer = image('d');
        assert!(!sandbox
            .check_daily_runtime_update(&newer, "2026-10-23T18:00:00Z")
            .unwrap());
        assert_eq!(sandbox, paused);

        // Both the manual and ordinary child-admission paths may observe a
        // newer global release. Neither may turn a cold canary into a managed home.
        for cause in [
            SandboxReviveCause::ExplicitRequest,
            SandboxReviveCause::AgentCreation,
        ] {
            let mut resumed = paused.clone();
            if cause == SandboxReviveCause::ExplicitRequest {
                resumed
                    .begin_revive_with_managed_runtime(cause, Some(&newer), NOW.into())
                    .unwrap();
            } else {
                assert_eq!(
                    resumed
                        .record_creation_admission_with_managed_runtime(
                            cause,
                            Some(&newer),
                            NOW.into(),
                        )
                        .unwrap(),
                    Some(SandboxEventKind::StatusChanged {
                        from: SandboxStatus::Paused,
                        to: SandboxStatus::Reviving,
                    }),
                );
            }
            assert!(resumed.runtime_update_request().is_none());
            assert_eq!(resumed.admitted_runtime_update_target(), None);
            assert_eq!(
                resumed.custom_runtime_image_for_binding().unwrap(),
                Some(pinned.as_str())
            );
            provision(&mut resumed, digit);
            finish(&mut resumed);
            assert_eq!(resumed.status, SandboxStatus::Active);
            assert_eq!(resumed.profile, original.profile);
            assert_eq!(resumed.lifecycle.home, original.lifecycle.home);
            assert_eq!(resumed.runtime.as_ref().unwrap().runtime_version, pinned);
            assert_eq!(
                resumed.runtime.as_ref().unwrap().runtime_id,
                original.runtime.as_ref().unwrap().runtime_id
            );
            assert!(resumed.runtime_update_request().is_none());
        }
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
