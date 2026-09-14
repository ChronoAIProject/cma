use crate::sandbox::{AgentStatus, FirstMessageDelivery};
use crate::{AgentId, SandboxId, ServiceId, SubjectId, WorkspaceId};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const LATEST: &str = "2026-09-05T00:02:00Z";

fn launch(resolution: LaunchResolution) -> Launch {
    let mut launch = Launch::new(LaunchSeed {
        trigger_id: TriggerId("trg_worker_control".into()),
        revision_digest: "a".repeat(64),
        public_id: PublicTriggerId("pub_worker_control".into()),
        owner: SubjectId::new("owner"),
        authority: LaunchAuthorityClass::SandboxCreator,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: vec![ServiceRequirement {
                service: ServiceId::new("github"),
                state: ServiceRequirementState::Satisfied,
            }],
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: Vec::new(),
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution,
        has_first_message: false,
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

fn pending_launch() -> Launch {
    launch(LaunchResolution::pending())
}

fn claim(launch: &mut Launch, holder: &str, now: &str, expires_at: &str) -> u64 {
    launch
        .work
        .claim(holder.into(), now, expires_at.into())
        .unwrap()
}

fn apply_effect(
    launch: &mut Launch,
    holder: &str,
    fence: u64,
    kind: LaunchEffectKind,
    result_id: &str,
    at: &str,
) {
    launch
        .prepare_worker_effect(holder, fence, kind, at.into())
        .unwrap();
    launch
        .mark_worker_effect_executing(holder, fence, at.into())
        .unwrap();
    launch
        .mark_worker_effect_applied(holder, fence, result_id.into(), at.into())
        .unwrap();
}

fn assert_rejected(
    launch: &Launch,
    before: &Launch,
    result: Result<Vec<LaunchEventKind>, TriggerError>,
) {
    assert_eq!(result.unwrap_err().code, "invalid_launch_worker_progress");
    assert_eq!(launch, before);
}

#[test]
fn wakeable_blocks_refresh_only_the_frozen_requirement_and_can_reblock() {
    let mut launch = pending_launch();
    let review = launch.review.clone();
    let confirmations = launch.confirmations.clone();
    let fence = claim(&mut launch, "worker", NOW, LATER);

    let before = launch.clone();
    let mismatch = launch.block_worker_launch(
        "worker",
        fence,
        LaunchBlockedReason::ConnectionRequired {
            service: ServiceId::new("github"),
        },
        Some(LaunchRequirementRefresh {
            service: ServiceId::new("slack"),
            state: ServiceRequirementState::Missing,
            checked_at: NOW.into(),
        }),
        LATER.into(),
        NOW.into(),
    );
    assert_rejected(&launch, &before, mismatch);

    let revision = launch.revision;
    let reason = LaunchBlockedReason::ConnectionRequired {
        service: ServiceId::new("github"),
    };
    let events = launch
        .block_worker_launch(
            "worker",
            fence,
            reason.clone(),
            Some(LaunchRequirementRefresh {
                service: ServiceId::new("github"),
                state: ServiceRequirementState::Missing,
                checked_at: NOW.into(),
            }),
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert!(!launch.is_worker_due(NOW));
    assert!(launch.is_worker_due(LATER));
    assert_eq!(launch.review, review);
    assert_eq!(launch.confirmations, confirmations);
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::RequirementChanged { state, .. },
            LaunchEventKind::StepChanged { step: accounts },
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::Blocked { .. },
            LaunchEventKind::Deferred { retry_at }
        ] if *state == ServiceRequirementState::Missing
            && accounts.key == LaunchStepKey::CheckingRequirements
            && accounts.status == LaunchStepStatus::Blocked
            && accounts.completed_at.is_none()
            && step.status == LaunchStepStatus::Blocked
            && retry_at == LATER
    ));

    let fence = claim(&mut launch, "worker", LATER, LATEST);
    let revision = launch.revision;
    let events = launch
        .resume_worker_launch(
            "worker",
            fence,
            reason,
            Some(LaunchRequirementRefresh {
                service: ServiceId::new("github"),
                state: ServiceRequirementState::Satisfied,
                checked_at: LATER.into(),
            }),
            LATER.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert!(launch.blocked_reason.is_none());
    assert!(launch.work.owns("worker", fence));
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::RequirementChanged { state, .. },
            LaunchEventKind::StepChanged { step: accounts },
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::Resumed { .. }
        ] if *state == ServiceRequirementState::Satisfied
            && accounts.key == LaunchStepKey::CheckingRequirements
            && accounts.status == LaunchStepStatus::Completed
            && accounts.completed_at.as_deref() == Some(LATER)
            && step.status == LaunchStepStatus::Running
    ));

    launch
        .block_worker_launch(
            "worker",
            fence,
            LaunchBlockedReason::OrchestrationPaused,
            None,
            LATEST.into(),
            LATER.into(),
        )
        .unwrap();
    assert_eq!(
        launch.blocked_reason,
        Some(LaunchBlockedReason::OrchestrationPaused)
    );
    assert!(!launch.is_worker_due(LATER));
    assert!(launch.is_worker_due(LATEST));
    launch.validate().unwrap();
}

#[test]
fn retry_schedule_is_bounded_and_exhaustion_fails_terminally() {
    let mut launch = pending_launch();
    let fence = claim(&mut launch, "worker", NOW, LATER);
    let before = launch.clone();
    let result = launch.retry_worker_launch(
        "worker",
        fence,
        "runtime_unavailable".into(),
        "Agent runtime is unavailable".into(),
        "2026-09-05T00:15:01Z".into(),
        NOW.into(),
    );
    assert_rejected(&launch, &before, result);

    for expected_attempt in 1..MAX_LAUNCH_WORK_ATTEMPTS {
        let fence = launch
            .work
            .lease
            .as_ref()
            .map(|lease| lease.fence)
            .unwrap_or_else(|| claim(&mut launch, "worker", NOW, LATER));
        let revision = launch.revision;
        let events = launch
            .retry_worker_launch(
                "worker",
                fence,
                "runtime_unavailable".into(),
                "Agent runtime is unavailable".into(),
                LATER.into(),
                NOW.into(),
            )
            .unwrap();
        assert_eq!(launch.revision, revision + 1);
        assert_eq!(launch.work.attempts, expected_attempt);
        assert!(matches!(
            events.last(),
            Some(LaunchEventKind::RetryScheduled { attempt, .. })
                if *attempt == expected_attempt
        ));
        launch.work.ready_at = NOW.into();
    }

    let fence = claim(&mut launch, "worker", NOW, LATER);
    let revision = launch.revision;
    let events = launch
        .retry_worker_launch(
            "worker",
            fence,
            "runtime_unavailable".into(),
            "Agent runtime is unavailable".into(),
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert_eq!(launch.state, LaunchState::FailedTerminal);
    assert_eq!(launch.work.step, LaunchWorkStep::None);
    assert!(matches!(
        events.as_slice(),
        [
            LaunchEventKind::StepChanged { step },
            LaunchEventKind::StateChanged {
                to: LaunchState::FailedTerminal,
                ..
            },
            LaunchEventKind::Failed { failure }
        ] if step.status == LaunchStepStatus::Failed && !failure.retryable
    ));
    launch.validate().unwrap();
}

#[test]
fn transient_executing_effect_retry_preserves_recovery_for_the_next_fence() {
    let mut launch = pending_launch();
    let first = claim(&mut launch, "worker-a", NOW, LATER);
    launch
        .prepare_worker_effect(
            "worker-a",
            first,
            LaunchEffectKind::CreateSandbox,
            NOW.into(),
        )
        .unwrap();
    launch
        .mark_worker_effect_executing("worker-a", first, NOW.into())
        .unwrap();

    let revision = launch.revision;
    let events = launch
        .retry_worker_launch(
            "worker-a",
            first,
            "sandbox_service_unavailable".into(),
            "Sandbox creation could not be confirmed".into(),
            LATER.into(),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(launch.revision, revision + 1);
    assert_eq!(launch.work.attempts, 1);
    assert!(launch.work.lease.is_none());
    assert_eq!(
        launch.work.effect.as_ref().unwrap().state,
        LaunchEffectState::RecoveryRequired
    );
    assert!(!launch.is_worker_due(NOW));
    assert!(launch.is_worker_due(LATER));
    assert!(matches!(
        events.last(),
        Some(LaunchEventKind::RetryScheduled { attempt: 1, .. })
    ));

    let second = claim(&mut launch, "worker-b", LATER, LATEST);
    let effect = launch.work.effect.as_ref().unwrap();
    assert!(second > first);
    assert_eq!(effect.fence, second);
    assert_eq!(effect.state, LaunchEffectState::RecoveryRequired);
    launch
        .reconcile_worker_effect(
            "worker-b",
            second,
            LaunchEffectKind::CreateSandbox,
            "sbx_replayed".into(),
            LATER.into(),
        )
        .unwrap();
    launch.validate().unwrap();
}

#[test]
fn terminal_failure_preserves_effect_authority_after_exhaustion_or_direct_failure() {
    for exhausted in [false, true] {
        for state in [
            LaunchEffectState::Prepared,
            LaunchEffectState::Executing,
            LaunchEffectState::RecoveryRequired,
            LaunchEffectState::Applied,
        ] {
            let mut launch = pending_launch();
            if exhausted {
                launch.work.attempts = MAX_LAUNCH_WORK_ATTEMPTS - 1;
            }
            let fence = claim(&mut launch, "worker", NOW, LATER);
            launch
                .prepare_worker_effect("worker", fence, LaunchEffectKind::CreateSandbox, NOW.into())
                .unwrap();
            if state != LaunchEffectState::Prepared {
                launch
                    .mark_worker_effect_executing("worker", fence, NOW.into())
                    .unwrap();
            }
            match state {
                LaunchEffectState::RecoveryRequired => launch
                    .work
                    .mark_effect_recovery_required("worker", fence, NOW.into())
                    .unwrap(),
                LaunchEffectState::Applied => launch
                    .mark_worker_effect_applied(
                        "worker",
                        fence,
                        "sbx_created_but_unresolved".into(),
                        NOW.into(),
                    )
                    .unwrap(),
                _ => {}
            }
            let before = launch.work.effect.clone().unwrap();
            if exhausted {
                launch
                    .retry_worker_launch(
                        "worker",
                        fence,
                        "service_unavailable".into(),
                        "Resource creation could not be confirmed".into(),
                        LATER.into(),
                        NOW.into(),
                    )
                    .unwrap();
            } else {
                launch
                    .fail_worker_launch(
                        "worker",
                        fence,
                        "invalid_resource_result".into(),
                        "Resource creation returned an invalid result".into(),
                        NOW.into(),
                    )
                    .unwrap();
            }
            assert_eq!(launch.state, LaunchState::FailedTerminal);
            assert_eq!(launch.work.step, LaunchWorkStep::None);
            assert!(launch.work.lease.is_none());
            assert!(!launch.is_worker_due(LATEST));
            assert!(launch.resolution.sandbox.is_pending());
            let mut expected = before;
            if expected.state == LaunchEffectState::Executing {
                expected.state = LaunchEffectState::RecoveryRequired;
            }
            assert_eq!(
                launch.work.effect.as_ref(),
                Some(&expected),
                "exhausted={exhausted}, state={state:?}"
            );
            let restored: Launch =
                serde_json::from_slice(&serde_json::to_vec(&launch).unwrap()).unwrap();
            restored.validate().unwrap();
            assert_eq!(restored, launch);
            let before = launch.clone();
            assert!(launch
                .mark_worker_effect_applied("worker", fence, "sbx_stale".into(), NOW.into())
                .is_err());
            assert_eq!(launch, before);
        }
    }
}

#[test]
fn ready_launch_observes_only_its_resolved_terminal_agent() {
    for status in [AgentStatus::Stopped, AgentStatus::Deleted] {
        let agent_id = AgentId("agt_ready".into());
        let resolution = LaunchResolution {
            sandbox: ResolvedResource::Reused(SandboxId("sbx_ready".into())),
            workspace: ResolvedResource::Reused(WorkspaceId("wks_ready".into())),
            agent: ResolvedResource::Reused(agent_id.clone()),
            choice_required: false,
        };
        let mut launch = launch(resolution);
        launch.first_message = FirstMessageDelivery::Completed;
        for step in &mut launch.steps {
            step.status = LaunchStepStatus::Completed;
            step.started_at.get_or_insert_with(|| NOW.into());
            step.completed_at.get_or_insert_with(|| NOW.into());
        }
        launch.transition(LaunchState::Ready, NOW.into()).unwrap();
        launch.validate().unwrap();
        assert_eq!(launch.work.step, LaunchWorkStep::Finish);
        let fence = claim(&mut launch, "observer", NOW, LATER);

        let before = launch.clone();
        let live = launch.complete_worker_launch(
            "observer",
            fence,
            &agent_id,
            AgentStatus::Ready,
            NOW.into(),
        );
        assert_rejected(&launch, &before, live);

        let revision = launch.revision;
        let events = launch
            .complete_worker_launch("observer", fence, &agent_id, status, NOW.into())
            .unwrap();
        assert_eq!(launch.revision, revision + 1);
        assert_eq!(launch.state, LaunchState::Completed);
        assert_eq!(launch.work.step, LaunchWorkStep::None);
        assert!(matches!(
            events.as_slice(),
            [
                LaunchEventKind::StateChanged {
                    from: LaunchState::Ready,
                    to: LaunchState::Completed
                },
                LaunchEventKind::Completed
            ]
        ));
    }
}

#[test]
fn crash_after_executing_reconciles_the_exact_effect_under_the_new_fence() {
    let mut launch = pending_launch();
    let first = claim(&mut launch, "worker-a", NOW, LATER);
    launch
        .prepare_worker_effect(
            "worker-a",
            first,
            LaunchEffectKind::CreateSandbox,
            NOW.into(),
        )
        .unwrap();
    launch
        .mark_worker_effect_executing("worker-a", first, NOW.into())
        .unwrap();
    let second = claim(&mut launch, "worker-b", LATER, LATEST);
    launch.updated_at = LATER.into();
    launch.revision += 1;
    assert_eq!(
        launch.work.effect.as_ref().unwrap().state,
        LaunchEffectState::RecoveryRequired
    );
    assert_eq!(launch.work.effect.as_ref().unwrap().fence, second);

    for (fence, kind, id) in [
        (first, LaunchEffectKind::CreateSandbox, "sbx_created"),
        (second, LaunchEffectKind::CreateWorkspace, "sbx_created"),
        (second, LaunchEffectKind::CreateSandbox, "wks_wrong"),
    ] {
        let before = launch.clone();
        let result =
            launch.reconcile_worker_effect("worker-b", fence, kind, id.into(), LATER.into());
        assert_rejected(&launch, &before, result);
    }

    let events = launch
        .reconcile_worker_effect(
            "worker-b",
            second,
            LaunchEffectKind::CreateSandbox,
            "sbx_created".into(),
            LATER.into(),
        )
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [LaunchEventKind::EffectReconciled {
            kind: LaunchEffectKind::CreateSandbox,
            result_id
        }] if result_id == "sbx_created"
    ));
    launch
        .record_worker_sandbox(
            "worker-b",
            second,
            SandboxId("sbx_created".into()),
            false,
            LATER.into(),
        )
        .unwrap();
    assert_eq!(
        launch.resolution.sandbox,
        ResolvedResource::Created(SandboxId("sbx_created".into()))
    );
}

#[test]
fn reused_resource_replacement_is_exact_and_clears_dependent_resolution() {
    let old_sandbox = SandboxId("sbx_old".into());
    let old_workspace = WorkspaceId("wks_old".into());
    let old_agent = AgentId("agt_old".into());
    let resolution = LaunchResolution {
        sandbox: ResolvedResource::Reused(old_sandbox.clone()),
        workspace: ResolvedResource::Reused(old_workspace.clone()),
        agent: ResolvedResource::Reused(old_agent),
        choice_required: false,
    };
    let mut launch = launch(resolution);
    let fence = claim(&mut launch, "sandbox", NOW, LATER);
    launch
        .record_worker_sandbox("sandbox", fence, old_sandbox.clone(), true, NOW.into())
        .unwrap();
    let fence = claim(&mut launch, "sandbox", NOW, LATER);
    apply_effect(
        &mut launch,
        "sandbox",
        fence,
        LaunchEffectKind::CreateSandbox,
        "sbx_new",
        NOW,
    );
    let before = launch.clone();
    let mismatch = launch.replace_worker_reused_sandbox(
        "sandbox",
        fence,
        &SandboxId("sbx_other".into()),
        SandboxId("sbx_new".into()),
        NOW.into(),
    );
    assert_rejected(&launch, &before, mismatch);
    launch
        .replace_worker_reused_sandbox(
            "sandbox",
            fence,
            &old_sandbox,
            SandboxId("sbx_new".into()),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(
        launch.resolution.sandbox,
        ResolvedResource::Created(SandboxId("sbx_new".into()))
    );
    assert!(launch.resolution.workspace.is_pending());
    assert!(launch.resolution.agent.is_pending());
    let fence = claim(&mut launch, "sandbox", NOW, LATER);
    let before = launch.clone();
    let repeated = launch.replace_worker_reused_sandbox(
        "sandbox",
        fence,
        &old_sandbox,
        SandboxId("sbx_newer".into()),
        NOW.into(),
    );
    assert_rejected(&launch, &before, repeated);

    let old_workspace = WorkspaceId("wks_old".into());
    let resolution = LaunchResolution {
        sandbox: ResolvedResource::Reused(SandboxId("sbx_kept".into())),
        workspace: ResolvedResource::Reused(old_workspace.clone()),
        agent: ResolvedResource::Reused(AgentId("agt_cleared".into())),
        choice_required: false,
    };
    let mut workspace_launch = self::launch(resolution);
    let fence = claim(&mut workspace_launch, "sandbox", NOW, LATER);
    workspace_launch
        .record_worker_sandbox(
            "sandbox",
            fence,
            SandboxId("sbx_kept".into()),
            true,
            NOW.into(),
        )
        .unwrap();
    let fence = claim(&mut workspace_launch, "sandbox", NOW, LATER);
    workspace_launch
        .record_worker_sandbox_ready("sandbox", fence, None, NOW.into())
        .unwrap();
    let fence = claim(&mut workspace_launch, "workspace", NOW, LATER);
    workspace_launch
        .record_worker_workspace("workspace", fence, old_workspace.clone(), true, NOW.into())
        .unwrap();
    let fence = claim(&mut workspace_launch, "workspace", NOW, LATER);
    apply_effect(
        &mut workspace_launch,
        "workspace",
        fence,
        LaunchEffectKind::CreateWorkspace,
        "wks_new",
        NOW,
    );
    workspace_launch
        .replace_worker_reused_workspace(
            "workspace",
            fence,
            &old_workspace,
            WorkspaceId("wks_new".into()),
            NOW.into(),
        )
        .unwrap();
    assert_eq!(
        workspace_launch.resolution.workspace,
        ResolvedResource::Created(WorkspaceId("wks_new".into()))
    );
    assert!(workspace_launch.resolution.agent.is_pending());
    workspace_launch.validate().unwrap();
}
