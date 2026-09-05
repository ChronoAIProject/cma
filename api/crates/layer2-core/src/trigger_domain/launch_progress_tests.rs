use crate::sandbox::{FirstMessageDelivery, FirstMessageFailure, FirstMessageRecord};
use crate::{AgentId, SandboxId, SubjectId, WorkspaceId};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const LATEST: &str = "2026-09-05T00:02:00Z";
const FINAL: &str = "2026-09-05T00:03:00Z";

fn launch_with_first_message(has_first_message: bool) -> Launch {
    let mut launch = Launch::new(LaunchSeed {
        trigger_id: TriggerId("trg_progress".into()),
        revision_digest: "a".repeat(64),
        public_id: PublicTriggerId("pub_progress".into()),
        owner: SubjectId::new("owner"),
        authority: LaunchAuthorityClass::Visitor,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: Vec::new(),
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: Vec::new(),
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message,
        command_key_hash: "b".repeat(64),
        command_fingerprint: "c".repeat(64),
        created_at: NOW.into(),
    })
    .unwrap();
    launch
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: Vec::new(),
            confirmed_at: NOW.into(),
        })
        .unwrap();
    launch
}

fn launch() -> Launch {
    launch_with_first_message(true)
}

fn claim(launch: &mut Launch, holder: &str) -> u64 {
    launch.work.claim(holder.into(), NOW, LATER.into()).unwrap()
}

fn claim_at(launch: &mut Launch, holder: &str, now: &str, expires_at: &str) -> u64 {
    launch
        .work
        .claim(holder.into(), now, expires_at.into())
        .unwrap()
}

fn apply_effect(launch: &mut Launch, holder: &str, fence: u64, kind: LaunchEffectKind, id: &str) {
    launch
        .prepare_worker_effect(holder, fence, kind, NOW.into())
        .unwrap();
    launch
        .mark_worker_effect_executing(holder, fence, NOW.into())
        .unwrap();
    launch
        .mark_worker_effect_applied(holder, fence, id.into(), NOW.into())
        .unwrap();
}

fn advance_to_agent_wait(launch: &mut Launch) {
    let sandbox_id = SandboxId("sbx_reused".into());
    let workspace_id = WorkspaceId("wks_reused".into());
    let agent_id = AgentId("agt_reused".into());

    let fence = claim(launch, "sandbox-resolver");
    launch
        .record_worker_sandbox("sandbox-resolver", fence, sandbox_id, true, NOW.into())
        .unwrap();
    let fence = claim(launch, "sandbox-waiter");
    launch
        .record_worker_sandbox_ready(
            "sandbox-waiter",
            fence,
            Some(workspace_id.clone()),
            NOW.into(),
        )
        .unwrap();
    let fence = claim(launch, "workspace-resolver");
    launch
        .record_worker_workspace("workspace-resolver", fence, workspace_id, true, NOW.into())
        .unwrap();
    let fence = claim(launch, "workspace-waiter");
    launch
        .record_worker_workspace_ready("workspace-waiter", fence, NOW.into())
        .unwrap();
    let fence = claim(launch, "agent-resolver");
    launch
        .record_worker_agent("agent-resolver", fence, agent_id, true, NOW.into())
        .unwrap();
}

fn message() -> FirstMessageRecord {
    FirstMessageRecord::new("launch-message", "Inspect the workspace", NOW).unwrap()
}

fn failure(at: &str) -> FirstMessageFailure {
    FirstMessageFailure::new(
        "first_message_delivery_failed",
        "the runtime did not confirm the write",
        false,
        at,
    )
    .unwrap()
}

fn resolved_agent(launch: &Launch) -> AgentId {
    launch.resolution.agent.id().unwrap().clone()
}

fn enter_message_pending(launch: &mut Launch, record: &FirstMessageRecord) -> AgentId {
    advance_to_agent_wait(launch);
    let agent_id = resolved_agent(launch);
    let fence = claim(launch, "agent-waiter");
    launch
        .record_worker_agent_ready("agent-waiter", fence, &agent_id, Some(record), NOW.into())
        .unwrap();
    agent_id
}

fn blocked_message_launch() -> (Launch, AgentId, FirstMessageRecord) {
    let mut launch = launch();
    let mut record = message();
    let agent_id = enter_message_pending(&mut launch, &record);
    record.fence_dispatch_for(11, 1, 7, NOW.into()).unwrap();
    let fence = claim(&mut launch, "message-observer");
    launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    record.require_recovery(failure(LATER)).unwrap();
    let fence = claim_at(&mut launch, "message-observer", LATER, LATEST);
    launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATEST.into(),
            LATER.into(),
        )
        .unwrap();
    (launch, agent_id, record)
}

fn assert_rejected_unchanged(
    launch: &Launch,
    before: &Launch,
    result: Result<Vec<LaunchEventKind>, TriggerError>,
) {
    assert_eq!(result.unwrap_err().code, "invalid_launch_worker_progress");
    assert_eq!(launch, before);
}

fn step(launch: &Launch, key: LaunchStepKey) -> &LaunchStep {
    launch.steps.iter().find(|step| step.key == key).unwrap()
}

#[test]
fn resource_progress_is_fenced_ordered_and_reevaluates_nested_workspace_reuse() {
    let mut launch = launch();
    let sandbox_id = SandboxId("sbx_progress".into());
    let workspace_id = WorkspaceId("wks_nested".into());
    let agent_id = AgentId("agt_progress".into());

    let fence = claim(&mut launch, "sandbox-worker");
    apply_effect(
        &mut launch,
        "sandbox-worker",
        fence,
        LaunchEffectKind::CreateSandbox,
        sandbox_id.as_str(),
    );
    let events = launch
        .record_worker_sandbox(
            "sandbox-worker",
            fence,
            sandbox_id.clone(),
            false,
            NOW.into(),
        )
        .unwrap();
    assert!(matches!(
        events.first(),
        Some(LaunchEventKind::SandboxResolved { reused: false, .. })
    ));
    assert_eq!(launch.state, LaunchState::SandboxPending);
    assert_eq!(launch.work.step, LaunchWorkStep::WaitForSandbox);

    let fence = claim(&mut launch, "sandbox-waiter");
    let events = launch
        .record_worker_sandbox_ready(
            "sandbox-waiter",
            fence,
            Some(workspace_id.clone()),
            NOW.into(),
        )
        .unwrap();
    assert!(matches!(
        events.first(),
        Some(LaunchEventKind::WorkspaceResolved { reused: true, .. })
    ));
    assert_eq!(
        launch.resolution.workspace,
        ResolvedResource::Reused(workspace_id.clone())
    );
    assert_eq!(launch.work.step, LaunchWorkStep::ResolveWorkspace);

    let fence = claim(&mut launch, "workspace-resolver");
    let events = launch
        .record_worker_workspace("workspace-resolver", fence, workspace_id, true, NOW.into())
        .unwrap();
    assert_eq!(
        events.len(),
        1,
        "the existing resolution is not emitted twice"
    );
    let fence = claim(&mut launch, "workspace-waiter");
    launch
        .record_worker_workspace_ready("workspace-waiter", fence, NOW.into())
        .unwrap();
    assert_eq!(launch.state, LaunchState::AgentPending);

    let fence = claim(&mut launch, "agent-worker");
    apply_effect(
        &mut launch,
        "agent-worker",
        fence,
        LaunchEffectKind::CreateAgent,
        agent_id.as_str(),
    );
    launch
        .record_worker_agent("agent-worker", fence, agent_id.clone(), false, NOW.into())
        .unwrap();
    assert_eq!(launch.resolution.agent, ResolvedResource::Created(agent_id));
    assert_eq!(launch.work.step, LaunchWorkStep::WaitForAgent);
    launch.validate().unwrap();
}

#[test]
fn resource_progress_rejects_an_uncorrelated_effect_result() {
    let mut launch = launch();
    let fence = claim(&mut launch, "worker");
    apply_effect(
        &mut launch,
        "worker",
        fence,
        LaunchEffectKind::CreateSandbox,
        "sbx_other",
    );
    assert_eq!(
        launch
            .record_worker_sandbox(
                "worker",
                fence,
                SandboxId("sbx_expected".into()),
                false,
                NOW.into(),
            )
            .unwrap_err()
            .code,
        "invalid_launch_worker_progress"
    );
    assert_eq!(launch.resolution.sandbox, ResolvedResource::Pending);
}

#[test]
fn agent_without_a_first_message_becomes_ready_immediately() {
    let mut launch = launch_with_first_message(false);
    advance_to_agent_wait(&mut launch);
    let agent_id = resolved_agent(&launch);
    let fence = claim(&mut launch, "agent-waiter");
    let revision = launch.revision;

    let events = launch
        .record_worker_agent_ready("agent-waiter", fence, &agent_id, None, LATER.into())
        .unwrap();

    assert_eq!(launch.revision, revision + 1);
    assert_eq!(launch.state, LaunchState::Ready);
    assert_eq!(launch.work.step, LaunchWorkStep::Finish);
    assert_eq!(launch.ready_at.as_deref(), Some(LATER));
    assert_eq!(
        step(&launch, LaunchStepKey::Agent).status,
        LaunchStepStatus::Completed
    );
    assert_eq!(
        step(&launch, LaunchStepKey::FirstMessage).status,
        LaunchStepStatus::Completed
    );
    assert_eq!(
        step(&launch, LaunchStepKey::Ready).status,
        LaunchStepStatus::Completed
    );
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::StepChanged { step: agent },
            LaunchEventKind::StateChanged {
                from: LaunchState::AgentPending,
                to: LaunchState::Ready
            },
            LaunchEventKind::StepChanged { step: ready }
        ] if agent.key == LaunchStepKey::Agent && ready.key == LaunchStepKey::Ready
    ));
    launch.validate().unwrap();
}

#[test]
fn durable_first_message_evidence_advances_to_ready_in_event_order() {
    let mut launch = launch();
    let mut record = message();
    advance_to_agent_wait(&mut launch);
    let agent_id = resolved_agent(&launch);
    let fence = claim(&mut launch, "agent-waiter");
    let revision = launch.revision;
    let events = launch
        .record_worker_agent_ready("agent-waiter", fence, &agent_id, Some(&record), NOW.into())
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::StepChanged { step: agent },
            LaunchEventKind::StateChanged {
                from: LaunchState::AgentPending,
                to: LaunchState::MessagePending
            },
            LaunchEventKind::StepChanged { step: message }
        ] if agent.key == LaunchStepKey::Agent
            && agent.status == LaunchStepStatus::Completed
            && message.key == LaunchStepKey::FirstMessage
            && message.status == LaunchStepStatus::Running
    ));
    assert_eq!(launch.state, LaunchState::MessagePending);
    assert_eq!(launch.work.step, LaunchWorkStep::DeliverFirstMessage);

    record.fence_dispatch_for(11, 1, 7, NOW.into()).unwrap();
    let fence = claim(&mut launch, "message-observer");
    let revision = launch.revision;
    let events = launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::FirstMessageChanged {
                state: FirstMessageDelivery::DispatchFenced
            },
            LaunchEventKind::StepChanged { step }
        ] if step.key == LaunchStepKey::FirstMessage
            && step.status == LaunchStepStatus::Running
    ));
    assert_eq!(launch.work.step, LaunchWorkStep::WaitForFirstMessage);

    record.mark_write_confirmed(LATER.into()).unwrap();
    let fence = claim_at(&mut launch, "message-observer", LATER, LATEST);
    let revision = launch.revision;
    let events = launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATEST.into(),
            LATER.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::FirstMessageChanged {
                state: FirstMessageDelivery::WriteConfirmed
            },
            LaunchEventKind::StepChanged { step }
        ] if step.key == LaunchStepKey::FirstMessage
            && step.status == LaunchStepStatus::Completed
    ));
    assert_eq!(launch.work.step, LaunchWorkStep::Finish);

    let fence = claim_at(&mut launch, "finisher", LATER, LATEST);
    let revision = launch.revision;
    launch
        .finish_worker_launch("finisher", fence, LATEST.into())
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert_eq!(launch.state, LaunchState::Ready);
    assert_eq!(launch.first_message, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(launch.work.step, LaunchWorkStep::Finish);
    launch.validate().unwrap();
}

#[test]
fn proven_pre_write_failure_returns_to_a_retryable_observation() {
    let mut launch = launch();
    let mut record = message();
    let agent_id = enter_message_pending(&mut launch, &record);
    record.fence_dispatch_for(11, 1, 7, NOW.into()).unwrap();
    let fence = claim(&mut launch, "message-observer");
    launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    record.retry_after_pre_write_failure().unwrap();

    let fence = claim_at(&mut launch, "message-observer", LATER, LATEST);
    let revision = launch.revision;
    let events = launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATEST.into(),
            LATER.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert_eq!(launch.first_message, FirstMessageDelivery::Queued);
    assert_eq!(launch.work.step, LaunchWorkStep::DeliverFirstMessage);
    assert_eq!(
        step(&launch, LaunchStepKey::FirstMessage).status,
        LaunchStepStatus::Retryable
    );
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::FirstMessageChanged {
                state: FirstMessageDelivery::Queued
            },
            LaunchEventKind::StepChanged { step }
        ] if step.status == LaunchStepStatus::Retryable
    ));

    record.fence_dispatch_for(11, 2, 8, LATEST.into()).unwrap();
    let fence = claim_at(&mut launch, "message-observer", LATEST, FINAL);
    launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            FINAL.into(),
            LATEST.into(),
        )
        .unwrap();
    assert_eq!(launch.first_message, FirstMessageDelivery::DispatchFenced);
    assert_eq!(launch.work.step, LaunchWorkStep::WaitForFirstMessage);
    launch.validate().unwrap();
}

#[test]
fn ambiguous_delivery_blocks_without_a_lease_or_stale_replay() {
    let (mut launch, agent_id, recovery) = blocked_message_launch();
    assert_eq!(launch.first_message, FirstMessageDelivery::RecoveryRequired);
    assert_eq!(
        launch.blocked_reason,
        Some(LaunchBlockedReason::RecoveryRequired)
    );
    assert!(launch.work.lease.is_none());
    assert_eq!(
        step(&launch, LaunchStepKey::FirstMessage).status,
        LaunchStepStatus::Blocked
    );
    assert_eq!(launch.first_message_evidence.as_ref().unwrap().attempt, 1);

    let mut stale_fenced = recovery.clone();
    stale_fenced.state = FirstMessageDelivery::DispatchFenced;
    stale_fenced.failed_at = None;
    stale_fenced.failure = None;
    stale_fenced.validate().unwrap();
    let before = launch.clone();
    let result = launch.reconcile_first_message_recovery(
        &agent_id,
        &stale_fenced,
        FINAL.into(),
        LATEST.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);

    let mut stale_write = stale_fenced;
    stale_write.mark_write_confirmed(LATEST.into()).unwrap();
    let before = launch.clone();
    let result = launch.reconcile_first_message_recovery(
        &agent_id,
        &stale_write,
        FINAL.into(),
        LATEST.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);
}

#[test]
fn late_positive_evidence_resolves_recovery_without_manual_resume() {
    let (mut launch, agent_id, mut record) = blocked_message_launch();
    record.mark_execution_observed(LATEST.into()).unwrap();
    let revision = launch.revision;

    let events = launch
        .reconcile_first_message_recovery(&agent_id, &record, FINAL.into(), LATEST.into())
        .unwrap();

    assert_eq!(launch.revision, revision + 1);
    assert_eq!(
        launch.first_message,
        FirstMessageDelivery::ExecutionObserved
    );
    assert_eq!(launch.work.step, LaunchWorkStep::Finish);
    assert!(launch.blocked_reason.is_none());
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::FirstMessageChanged {
                state: FirstMessageDelivery::ExecutionObserved
            },
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::Resumed {
                reason: LaunchBlockedReason::RecoveryRequired
            }
        ] if step.status == LaunchStepStatus::Completed
    ));
    launch.validate().unwrap();
}

#[test]
fn late_failed_evidence_settles_the_launch_terminally() {
    let (mut launch, agent_id, mut record) = blocked_message_launch();
    record.fail(failure(LATEST)).unwrap();
    let revision = launch.revision;

    let events = launch
        .reconcile_first_message_recovery(&agent_id, &record, FINAL.into(), LATEST.into())
        .unwrap();

    assert_eq!(launch.revision, revision + 1);
    assert_eq!(launch.state, LaunchState::FailedTerminal);
    assert_eq!(launch.first_message, FirstMessageDelivery::Failed);
    assert!(launch.blocked_reason.is_none());
    assert_eq!(launch.work.step, LaunchWorkStep::None);
    assert_eq!(
        launch.failure.as_ref().unwrap().code,
        "first_message_failed"
    );
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::FirstMessageChanged {
                state: FirstMessageDelivery::Failed
            },
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::StateChanged {
                from: LaunchState::MessagePending,
                to: LaunchState::FailedTerminal
            },
            LaunchEventKind::Failed { .. }
        ] if step.status == LaunchStepStatus::Failed
    ));
    launch.validate().unwrap();
}

#[test]
fn owner_authorized_recovery_generation_requeues_or_accepts_later_evidence() {
    let (mut launch, agent_id, mut record) = blocked_message_launch();
    let digest = record.payload_sha256.clone();
    record
        .authorize_recovery(&digest, true, LATEST.into())
        .unwrap();
    let revision = launch.revision;
    let events = launch
        .reconcile_first_message_recovery(&agent_id, &record, FINAL.into(), LATEST.into())
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert_eq!(record.recovery_generation, 1);
    assert_eq!(launch.first_message, FirstMessageDelivery::Queued);
    assert_eq!(launch.work.step, LaunchWorkStep::DeliverFirstMessage);
    assert!(launch.blocked_reason.is_none());
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::FirstMessageChanged {
                state: FirstMessageDelivery::Queued
            },
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::Resumed { .. }
        ] if step.status == LaunchStepStatus::Retryable
    ));

    let (mut launch, agent_id, mut record) = blocked_message_launch();
    let digest = record.payload_sha256.clone();
    record
        .authorize_recovery(&digest, true, LATEST.into())
        .unwrap();
    record.fence_dispatch_for(21, 2, 8, LATEST.into()).unwrap();
    record.mark_write_confirmed(LATEST.into()).unwrap();
    launch
        .reconcile_first_message_recovery(&agent_id, &record, FINAL.into(), LATEST.into())
        .unwrap();
    assert_eq!(launch.first_message, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(launch.work.step, LaunchWorkStep::Finish);
    assert!(launch.blocked_reason.is_none());
    launch.validate().unwrap();
}

#[test]
fn stale_message_identity_and_dispatch_cursors_leave_the_launch_unchanged() {
    let mut launch = launch();
    let mut record = message();
    let agent_id = enter_message_pending(&mut launch, &record);
    record.fence_dispatch_for(11, 1, 7, NOW.into()).unwrap();
    let fence = claim(&mut launch, "message-observer");
    launch
        .record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &record,
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    let fence = claim_at(&mut launch, "message-observer", LATER, LATEST);

    let mut variants = Vec::new();
    let mut other_origin =
        FirstMessageRecord::new("other-origin", "Inspect the workspace", NOW).unwrap();
    other_origin
        .fence_dispatch_for(11, 1, 7, NOW.into())
        .unwrap();
    variants.push(other_origin);
    let mut other_payload =
        FirstMessageRecord::new("launch-message", "Inspect something else", NOW).unwrap();
    other_payload
        .fence_dispatch_for(11, 1, 7, NOW.into())
        .unwrap();
    variants.push(other_payload);
    let mut generation = record.clone();
    generation.recovery_generation = 1;
    variants.push(generation);
    let mut attempt = record.clone();
    attempt.attempt = 2;
    variants.push(attempt);
    let mut message_fence = record.clone();
    message_fence.fence_sequence = 2;
    variants.push(message_fence);
    let mut dispatch = record.clone();
    dispatch.dispatch_generation = 8;
    variants.push(dispatch);
    let mut turn = record.clone();
    turn.turn_sequence = Some(12);
    variants.push(turn);

    let before = launch.clone();
    let result = launch.record_worker_first_message(
        "message-observer",
        fence,
        &AgentId("agt_other".into()),
        &record,
        LATEST.into(),
        LATER.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);
    for variant in variants {
        variant.validate().unwrap();
        let before = launch.clone();
        let result = launch.record_worker_first_message(
            "message-observer",
            fence,
            &agent_id,
            &variant,
            LATEST.into(),
            LATER.into(),
        );
        assert_rejected_unchanged(&launch, &before, result);
    }
}

#[test]
fn worker_message_commands_are_transactional_on_time_fence_and_revision_errors() {
    let mut launch = launch();
    let record = message();
    let agent_id = enter_message_pending(&mut launch, &record);
    let fence = claim(&mut launch, "message-observer");

    let before = launch.clone();
    let result = launch.record_worker_first_message(
        "message-observer",
        fence + 1,
        &agent_id,
        &record,
        LATER.into(),
        NOW.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);

    let before = launch.clone();
    let result = launch.record_worker_first_message(
        "message-observer",
        fence,
        &agent_id,
        &record,
        "2026-09-04T23:59:00Z".into(),
        NOW.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);

    launch.updated_at = LATER.into();
    let before = launch.clone();
    let result = launch.record_worker_first_message(
        "message-observer",
        fence,
        &agent_id,
        &record,
        LATEST.into(),
        NOW.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);

    launch.updated_at = NOW.into();
    launch.revision = u64::MAX;
    let before = launch.clone();
    let result = launch.record_worker_first_message(
        "message-observer",
        fence,
        &agent_id,
        &record,
        LATER.into(),
        NOW.into(),
    );
    assert_eq!(result.unwrap_err().code, "launch_revision_overflow");
    assert_eq!(launch, before);
}

#[test]
fn agent_readiness_rejects_the_wrong_agent_or_message_configuration_atomically() {
    let mut launch = launch();
    let record = message();
    advance_to_agent_wait(&mut launch);
    let agent_id = resolved_agent(&launch);
    let fence = claim(&mut launch, "agent-waiter");

    let before = launch.clone();
    let result = launch.record_worker_agent_ready(
        "agent-waiter",
        fence,
        &AgentId("agt_other".into()),
        Some(&record),
        NOW.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);

    let before = launch.clone();
    let result =
        launch.record_worker_agent_ready("agent-waiter", fence, &agent_id, None, NOW.into());
    assert_rejected_unchanged(&launch, &before, result);

    let mut unsafe_record = record;
    unsafe_record.origin_key = "unsafe\norigin".into();
    unsafe_record.validate().unwrap();
    let before = launch.clone();
    let result = launch.record_worker_agent_ready(
        "agent-waiter",
        fence,
        &agent_id,
        Some(&unsafe_record),
        NOW.into(),
    );
    assert_rejected_unchanged(&launch, &before, result);
}

#[test]
fn persisted_message_evidence_and_state_correlations_fail_closed() {
    let (launch, _, _) = blocked_message_launch();
    launch.validate().unwrap();

    let mut unsafe_origin = launch.clone();
    unsafe_origin
        .first_message_evidence
        .as_mut()
        .unwrap()
        .origin_key = "unsafe\norigin".into();
    assert_eq!(
        unsafe_origin.validate().unwrap_err().code,
        "invalid_launch_first_message"
    );

    let mut malformed_digest = launch.clone();
    malformed_digest
        .first_message_evidence
        .as_mut()
        .unwrap()
        .payload_sha256 = "A".repeat(64);
    assert_eq!(
        malformed_digest.validate().unwrap_err().code,
        "invalid_launch_first_message"
    );

    let mut wrong_agent = launch.clone();
    wrong_agent
        .first_message_evidence
        .as_mut()
        .unwrap()
        .agent_id = AgentId("agt_other".into());
    assert_eq!(
        wrong_agent.validate().unwrap_err().code,
        "invalid_launch_first_message"
    );

    let mut missing_cursor = launch.clone();
    missing_cursor
        .first_message_evidence
        .as_mut()
        .unwrap()
        .turn_sequence = None;
    assert_eq!(
        missing_cursor.validate().unwrap_err().code,
        "invalid_launch_first_message"
    );

    let mut unblocked_recovery = launch;
    unblocked_recovery.blocked_reason = None;
    assert_eq!(
        unblocked_recovery.validate().unwrap_err().code,
        "invalid_launch_first_message"
    );

    let (mut ready, agent_id, mut record) = blocked_message_launch();
    record.mark_execution_observed(LATEST.into()).unwrap();
    ready
        .reconcile_first_message_recovery(&agent_id, &record, FINAL.into(), LATEST.into())
        .unwrap();
    let fence = claim_at(&mut ready, "finisher", LATEST, FINAL);
    ready
        .finish_worker_launch("finisher", fence, FINAL.into())
        .unwrap();
    ready.validate().unwrap();
    ready.first_message = FirstMessageDelivery::Queued;
    ready.first_message_evidence = None;
    assert_eq!(
        ready.validate().unwrap_err().code,
        "invalid_launch_first_message"
    );
}

#[test]
fn provisioning_recovery_remains_effect_fenced_and_transactional() {
    let mut launch = launch();
    let fence = claim(&mut launch, "sandbox-worker");
    launch
        .prepare_worker_effect(
            "sandbox-worker",
            fence,
            LaunchEffectKind::CreateSandbox,
            NOW.into(),
        )
        .unwrap();
    launch
        .mark_worker_effect_executing("sandbox-worker", fence, NOW.into())
        .unwrap();
    let revision = launch.revision;
    let events = launch
        .block_worker_recovery("sandbox-worker", fence, LATER.into())
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert_eq!(
        launch.work.effect.as_ref().unwrap().state,
        LaunchEffectState::RecoveryRequired
    );
    assert!(launch.work.lease.is_none());
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::Blocked {
                reason: LaunchBlockedReason::RecoveryRequired
            }
        ] if step.status == LaunchStepStatus::Blocked
    ));

    let before = launch.clone();
    let result = launch.block_worker_recovery("sandbox-worker", fence, LATEST.into());
    assert_rejected_unchanged(&launch, &before, result);
}
