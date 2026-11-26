use crate::sandbox::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use crate::sandbox::store::{
    checkpoint_capture::validate_checkpoint_capture_work, AgentLogStore, AgentStore,
    AgentWorkerStore, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore, SandboxStoreError,
    SandboxTransaction, SandboxTransactionStore, SandboxWorkerStore, WorkspaceStore,
};
use crate::sandbox::*;

const LATER: &str = "2026-09-05T00:31:00Z";
const EXPIRES: &str = "2026-09-05T00:36:00Z";
const HOLDER: &str = "checkpoint-worker";

#[test]
fn capture_publication_preserves_credential_work_and_debt_until_exact_resume() {
    let mut value = active();
    value.work = SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, EXPIRES.into());
    let work = value.work.clone();
    value.begin_checkpoint_capture(NOW.into()).unwrap();
    let token = value
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .authority
        .writer_resume_token
        .clone();
    claim(&mut value);
    let claimed = value.clone();
    assert!(value
        .complete_checkpoint_writer_resume(HOLDER, fence(&value), LATER.into())
        .is_err());
    assert_eq!(value, claimed);
    value
        .publish_captured_checkpoint(checkpoint(1, NOW), HOLDER, fence(&value), LATER.into())
        .unwrap();
    assert_eq!(value.work, work);
    assert_eq!(
        value.lifecycle.checkpoint_capture.as_ref().unwrap().phase,
        SandboxCheckpointCapturePhase::ResumeWriters
    );
    assert_eq!(
        value
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority
            .writer_resume_token,
        token
    );
    assert_eq!(
        value.lifecycle.checkpoints.latest.as_ref().unwrap().epoch,
        1
    );
    let mut restored: Sandbox =
        serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
    claim(&mut restored);
    let original = restored.clone();
    assert!(restored
        .complete_checkpoint_writer_resume(HOLDER, fence(&restored) + 1, LATER.into())
        .is_err());
    assert_eq!(restored, original);
    restored
        .complete_checkpoint_writer_resume(HOLDER, fence(&restored), LATER.into())
        .unwrap();
    assert!(restored.lifecycle.checkpoint_capture.is_none());
    assert_eq!(restored.work, work);
}

#[test]
fn retirement_preserves_periodic_and_lifecycle_resume_authority() {
    let mut periodic = active();
    periodic.begin_checkpoint_capture(NOW.into()).unwrap();
    let held = periodic.lifecycle.checkpoint_capture.clone();
    periodic.begin_retirement(LATER.into()).unwrap();
    periodic.validate().unwrap();
    assert_eq!(periodic.lifecycle.checkpoint_capture, held);
    let mut lifecycle = active();
    lifecycle.begin_pause(NOW.into(), None).unwrap();
    let before = lifecycle.clone();
    let authority = lifecycle
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .authority()
        .clone();
    lifecycle.begin_retirement(LATER.into()).unwrap();
    lifecycle.validate().unwrap();
    assert!(lifecycle.lifecycle.operation.is_none());
    let capture = lifecycle.lifecycle.checkpoint_capture.as_ref().unwrap();
    assert_eq!(capture.authority, authority);
    assert_eq!(capture.phase, SandboxCheckpointCapturePhase::ResumeWriters);
    before
        .validate_checkpoint_capture_successor(&lifecycle)
        .unwrap();
    let mut erased = lifecycle.clone();
    erased.lifecycle.checkpoint_capture = None;
    assert!(lifecycle
        .validate_checkpoint_capture_successor(&erased)
        .is_err());
}

#[test]
fn lifecycle_retry_renews_only_confirmed_resumed_tokens_and_preserves_operation_identity() {
    let mut value = active();
    value.begin_pause(NOW.into(), None).unwrap();
    value
        .work
        .claim(HOLDER.into(), LATER, EXPIRES.into())
        .unwrap();
    let authority = value
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .authority()
        .clone();
    let fence = value.work.lease.as_ref().unwrap().fence;
    let current = value.clone();
    value
        .retry_lifecycle_after_writer_resume(LATER.into(), HOLDER, fence, LATER.into())
        .unwrap();
    let renewed = value.lifecycle.operation.as_ref().unwrap().authority();
    assert_eq!(renewed.operation_id, authority.operation_id);
    assert_eq!(renewed.writer_resume_generation, 1);
    assert_ne!(renewed.writer_resume_token, authority.writer_resume_token);
    let mut mutation = mutation(&current, value);
    mutation.events = vec![SandboxEventKind::WorkStepFailed {
        step: SandboxWorkStep::RunLifecycleOperation,
        failure: TypedFailure {
            class: "retryable".into(),
            detail: "checkpoint retry".into(),
        },
    }];
    crate::sandbox::store::validate_lifecycle_mutation(&current, &[], &mutation).unwrap();
}

#[test]
fn late_lifecycle_retry_requires_fresh_capture_for_pause_and_image_rotation() {
    for rotation in [false, true] {
        for parked in [false, true] {
            let mut value = active();
            if rotation {
                let identity = SandboxRotationIdentity::capture(&value, &[]).unwrap();
                value
                    .begin_image_rotation("image-v2".into(), identity, NOW.into())
                    .unwrap();
            } else {
                value.begin_pause(NOW.into(), None).unwrap();
            }
            let claim_work = |value: &mut Sandbox| {
                value
                    .work
                    .claim(HOLDER.into(), LATER, EXPIRES.into())
                    .unwrap();
                value.work.lease.as_ref().unwrap().fence
            };
            let fence = claim_work(&mut value);
            value
                .publish_lifecycle_checkpoint(checkpoint(1, NOW), HOLDER, fence, LATER.into())
                .unwrap();
            if parked {
                let fence = claim_work(&mut value);
                value
                    .complete_lifecycle_agent_pause(HOLDER, fence, LATER.into())
                    .unwrap();
            }
            let fence = claim_work(&mut value);
            let current = value.clone();
            let authority = current.lifecycle.operation.as_ref().unwrap().authority();
            assert!(value
                .retry_lifecycle_after_writer_resume(LATER.into(), HOLDER, fence + 1, LATER.into())
                .is_err());
            assert_eq!(value, current);
            value
                .retry_lifecycle_after_writer_resume(LATER.into(), HOLDER, fence, LATER.into())
                .unwrap();
            let renewed = value.lifecycle.operation.as_ref().unwrap();
            assert_eq!(renewed.authority().operation_id, authority.operation_id);
            assert_ne!(
                renewed.authority().writer_resume_token,
                authority.writer_resume_token
            );
            assert_eq!(renewed.authority().writer_resume_generation, 1);
            assert!(renewed.checkpoint_epoch().is_none());
            assert_eq!(value.lifecycle.checkpoints, current.lifecycle.checkpoints);
            let mut retry = mutation(&current, value.clone());
            retry.events = vec![SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure: TypedFailure {
                    class: "retryable".into(),
                    detail: "late retry".into(),
                },
            }];
            crate::sandbox::store::validate_lifecycle_mutation(&current, &[], &retry).unwrap();
            let mut forged = retry.clone();
            match forged.sandbox.lifecycle.operation.as_mut().unwrap() {
                SandboxLifecycleOperation::Pause {
                    checkpoint_epoch, ..
                }
                | SandboxLifecycleOperation::ImageRotation {
                    checkpoint_epoch, ..
                } => {
                    *checkpoint_epoch = Some(1);
                }
                _ => unreachable!(),
            }
            assert!(
                crate::sandbox::store::validate_lifecycle_mutation(&current, &[], &forged).is_err()
            );
            let mut forged = retry;
            forged.sandbox.lifecycle.checkpoints.latest = None;
            assert!(
                crate::sandbox::store::validate_lifecycle_mutation(&current, &[], &forged).is_err()
            );
            let fence = claim_work(&mut value);
            assert_eq!(value.next_lifecycle_checkpoint_epoch().unwrap(), 2);
            value
                .publish_lifecycle_checkpoint(checkpoint(2, LATER), HOLDER, fence, LATER.into())
                .unwrap();
            assert_eq!(
                value
                    .lifecycle
                    .operation
                    .as_ref()
                    .unwrap()
                    .checkpoint_epoch(),
                Some(2)
            );
            assert!(value.lifecycle.checkpoints.previous.is_none());
        }
    }
}

#[test]
fn only_exact_runtime_loss_discards_capture_debt_without_resume() {
    let mut value = active();
    value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
    value.begin_checkpoint_capture(LATER.into()).unwrap();
    value
        .recover_runtime_loss(&[], false, LATER.into())
        .unwrap();
    assert_eq!(value.status, SandboxStatus::Reviving);
    assert!(value.lifecycle.checkpoint_capture.is_none());
    assert!(value.runtime.is_none());
    value.validate().unwrap();
}

#[tokio::test]
async fn begin_and_resume_are_atomic_and_generic_writes_cannot_erase_debt() {
    let store = InMemorySandboxStore::default();
    let mut current = active();
    current.revision = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    let mut next = current.clone();
    next.begin_checkpoint_capture(LATER.into()).unwrap();
    let begin = mutation(&current, next.clone());
    store.commit_sandbox_lifecycle(begin.clone()).await.unwrap();
    assert_eq!(
        store.commit_sandbox_lifecycle(begin).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut erased = next.clone();
    erased.lifecycle.checkpoint_capture = None;
    erased.revision += 1;
    assert_eq!(
        store.save_sandbox(next.revision, erased.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: next.revision,
                sandbox: Box::new(erased),
                event_at: LATER.into(),
                event: SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation
                },
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert!(store
        .claim_sandbox_work(&next.id, HOLDER, LATER.into(), EXPIRES.into())
        .await
        .unwrap()
        .is_none());
    let mut claimed = next.clone();
    claim(&mut claimed);
    store
        .commit_sandbox_lifecycle(mutation(&next, claimed.clone()))
        .await
        .unwrap();
    let mut resumed = claimed.clone();
    resumed
        .require_checkpoint_writer_resume(HOLDER, fence(&claimed), LATER.into())
        .unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&claimed, resumed.clone()))
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(resumed));
}

#[tokio::test]
async fn begin_rejects_a_mismatched_conversation_log_tail_without_persisting_debt() {
    let store = InMemorySandboxStore::default();
    let mut current = active();
    current.revision = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    let mut writer = agent("agt_bad_tail", "capture-conversation", 1);
    writer.revision = 1;
    store.create_agent(writer).await.unwrap();
    let mut next = current.clone();
    next.begin_checkpoint_capture(LATER.into()).unwrap();
    assert_eq!(
        store
            .commit_sandbox_lifecycle(mutation(&current, next))
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(current));
}

#[tokio::test]
async fn begin_rejects_busy_or_expired_writer_authority_and_blocks_child_claims() {
    let store = InMemorySandboxStore::default();
    let mut current = active();
    current.revision = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    let mut writer = agent("agt_capture", "capture-conversation", 0);
    writer.revision = 1;
    writer.work = AgentWorkState::new(AgentWorkStep::OpenConversation, NOW.into());
    writer.work.claim(HOLDER.into(), NOW, LATER.into()).unwrap();
    assert!(validate_checkpoint_capture_work(&current, &[writer.clone()], &[]).is_err());
    store.create_agent(writer.clone()).await.unwrap();
    let mut next = current.clone();
    next.begin_checkpoint_capture(LATER.into()).unwrap();
    assert_eq!(
        store
            .commit_sandbox_lifecycle(mutation(&current, next))
            .await,
        Err(SandboxStoreError::Conflict)
    );
    writer.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    writer.conversation.last_seq = 1;
    store
        .append_agent_log(
            &writer.id,
            "2026-09-05T00:28:00Z".into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        )
        .await
        .unwrap();
    writer.revision += 1;
    store.save_agent(1, writer.clone()).await.unwrap();
    let mut next = current.clone();
    next.begin_checkpoint_capture(LATER.into()).unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&current, next))
        .await
        .unwrap();
    writer
        .work
        .schedule(AgentWorkStep::OpenConversation, LATER.into());
    writer.revision += 1;
    store.save_agent(2, writer.clone()).await.unwrap();
    assert!(store
        .claim_agent_work(&writer.id, HOLDER, LATER.into(), EXPIRES.into())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn begin_waits_for_pinned_profile_agents_before_fencing_a_ready_workspace() {
    let store = InMemorySandboxStore::default();
    let mut current = active();
    current.revision = 1;
    let mut child = agent("agt_profile_capture", "profile-conversation", 0);
    child.revision = 1;
    let workspace = profile_workspace(&current, &child);
    child.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    child.creation_key = crate::sandbox::agent::profile_agent_creation_key(&workspace.id, 0);
    store.create_sandbox(current.clone()).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    let mut next = current.clone();
    next.begin_checkpoint_capture(LATER.into()).unwrap();
    assert_eq!(
        store
            .commit_sandbox_lifecycle(mutation(&current, next.clone()))
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    child.conversation.last_seq = 1;
    store.create_agent(child.clone()).await.unwrap();
    store
        .append_agent_log(
            &child.id,
            "2026-09-05T00:28:00Z".into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        )
        .await
        .unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&current, next))
        .await
        .unwrap();
}

#[test]
fn profile_capture_readiness_accepts_trigger_reuse_but_not_unrelated_agents() {
    let current = active();
    let mut child = agent("agt_profile_capture", "profile-conversation", 0);
    let workspace = profile_workspace(&current, &child);
    let eligible = |child: &Agent, workspace: &Workspace| {
        validate_checkpoint_capture_work(
            &current,
            std::slice::from_ref(child),
            std::slice::from_ref(workspace),
        )
        .is_ok()
    };
    assert!(!eligible(&child, &workspace));
    child.origin = AgentOrigin::Trigger {
        trigger_id: "trg_profile".into(),
        launch_id: "lch_profile".into(),
    };
    assert!(!eligible(&child, &workspace));
    child.creation_key = "trigger-profile:launch:agent".into();
    assert!(eligible(&child, &workspace));
    child.profile.revision += 1;
    assert!(!eligible(&child, &workspace));
    child.profile.revision -= 1;
    child.workspace_id = crate::WorkspaceId("wks_other".into());
    assert!(!eligible(&child, &workspace));
    child.workspace_id = workspace.id.clone();
    let mut duplicate = workspace.clone();
    duplicate
        .profile
        .content
        .agent_profiles
        .push(duplicate.profile.content.agent_profiles[0].clone());
    assert!(!eligible(&child, &duplicate));
    for origin in [
        child.origin.clone(),
        AgentOrigin::WorkspaceProfile { index: 0 },
    ] {
        child.origin = origin;
        if matches!(child.origin, AgentOrigin::WorkspaceProfile { .. }) {
            child.creation_key =
                crate::sandbox::agent::profile_agent_creation_key(&workspace.id, 0);
        }
        for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
            child.status = status;
            assert!(eligible(&child, &workspace));
        }
    }
}

fn profile_workspace(sandbox: &Sandbox, child: &Agent) -> Workspace {
    use crate::profiles::{
        AgentProfileRef, WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
    };
    Workspace {
        id: child.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: crate::WorkspaceProfileId("wkp_capture".into()),
            revision: 1,
            digest: "e".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "capture".into(),
                },
                name: "capture".into(),
                setup_commands: Vec::new(),
                agent_profiles: vec![AgentProfileRef {
                    id: child.profile.id.clone(),
                    revision: child.profile.revision,
                }],
            },
        },
        name: "capture".into(),
        path: "/home/cma/capture".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("f".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn active() -> Sandbox {
    let mut value = sandbox();
    value.runtime.as_mut().unwrap().provision_generation = Some(1);
    value.lifecycle.runtime_generation_high_water = 1;
    value
}

#[tokio::test]
async fn checkpoint_renewal_is_exact_fenced_unexpired_and_capture_only() {
    let store = InMemorySandboxStore::default();
    let mut current = active();
    current.revision = 1;
    store.create_sandbox(current.clone()).await.unwrap();
    let mut begun = current.clone();
    begun.begin_checkpoint_capture(LATER.into()).unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&current, begun.clone()))
        .await
        .unwrap();
    let mut claimed = begun.clone();
    claim(&mut claimed);
    store
        .commit_sandbox_lifecycle(mutation(&begun, claimed.clone()))
        .await
        .unwrap();
    let at = "2026-09-05T00:32:00Z";
    let expires = "2026-09-05T00:37:00Z";
    let mut renewed = claimed.clone();
    renewed
        .renew_checkpoint_capture(HOLDER, fence(&claimed), at.into(), expires.into())
        .unwrap();
    let mut valid = mutation(&claimed, renewed.clone());
    valid.appended_at = at.into();
    for change in [0, 1, 2] {
        let mut invalid = valid.clone();
        let capture = invalid
            .sandbox
            .lifecycle
            .checkpoint_capture
            .as_mut()
            .unwrap();
        match change {
            0 => capture.phase = SandboxCheckpointCapturePhase::ResumeWriters,
            1 => capture.authority.writer_resume_token = Some("a".repeat(64)),
            _ => capture.work.attempts += 1,
        }
        assert!(store.commit_sandbox_lifecycle(invalid).await.is_err());
        assert_eq!(
            store.get_sandbox(&current.id).await.unwrap(),
            Some(claimed.clone())
        );
    }
    store.commit_sandbox_lifecycle(valid.clone()).await.unwrap();
    assert_eq!(
        store.commit_sandbox_lifecycle(valid).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(fence(&renewed), fence(&claimed));
    assert_eq!(
        renewed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority,
        claimed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority
    );
    assert_eq!(
        renewed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .work
            .attempts,
        0
    );
    for (holder, lease_fence, at, until) in [
        ("other", fence(&claimed), at, "2026-09-05T00:38:00Z"),
        (HOLDER, fence(&claimed) + 1, at, "2026-09-05T00:38:00Z"),
        (HOLDER, fence(&claimed), expires, "2026-09-05T00:38:00Z"),
        (HOLDER, fence(&claimed), at, EXPIRES),
    ] {
        let mut value = renewed.clone();
        assert!(value
            .renew_checkpoint_capture(holder, lease_fence, at.into(), until.into())
            .is_err());
        assert_eq!(value, renewed);
    }
    let mut resuming = renewed.clone();
    resuming
        .require_checkpoint_writer_resume(HOLDER, fence(&renewed), at.into())
        .unwrap();
    resuming
        .claim_checkpoint_capture(HOLDER.into(), at.into(), expires.into())
        .unwrap();
    let before = resuming.clone();
    assert!(resuming
        .renew_checkpoint_capture(HOLDER, fence(&before), at.into(), expires.into())
        .is_err());
    assert_eq!(resuming, before);
}

fn claim(value: &mut Sandbox) {
    value
        .claim_checkpoint_capture(HOLDER.into(), LATER.into(), EXPIRES.into())
        .unwrap();
}

fn fence(value: &Sandbox) -> u64 {
    value
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence
}

fn mutation(current: &Sandbox, sandbox: Sandbox) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox,
        agents: Vec::new(),
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: LATER.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}
