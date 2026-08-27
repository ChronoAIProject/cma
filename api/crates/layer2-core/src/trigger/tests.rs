use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde_json::json;
use sha2::Digest as _;

use super::*;
use crate::{seconds_from_rfc3339, PermissionMode, SessionId, SessionSize, SubjectId};

const NOW: &str = "2026-08-26T12:00:00Z";
const LATER: &str = "2026-08-26T12:01:00Z";

pub(crate) fn session_launch(archive_sha256: &str) -> Launch {
    let owner = SubjectId("trigger-contributor".into());
    let mut launch = Launch::new(
        owner,
        revision(SubjectId("trigger-publisher".into())),
        NOW,
        "create-session-fixture",
    )
    .unwrap();
    launch
        .select_connection(connection(), NOW, "select-session-connection")
        .unwrap();
    let baseline = WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap();
    launch.workspace = Some(WorkspaceRecord {
        baseline,
        baseline_object_reference: "trigger-test/baseline.pack".into(),
        baseline_archive_sha256: archive_sha256.into(),
        latest_checkpoint: None,
        previous_checkpoint: None,
        workspace_epoch: 1,
        retention_policy_id: "trigger-test-retention".into(),
        delete_after: "2026-09-26T12:00:00Z".into(),
    });
    launch.state = LaunchState::SandboxPending;
    launch
}

pub(crate) fn prompt_launch(mut launch: Launch, session: &crate::Session) -> (Launch, EffectClaim) {
    launch.fork = Some(ForkRecord {
        repository: fork(),
        reused: false,
        source_revision: launch.source_sha.clone(),
        canonical_url: "https://github.com/contributor/project".into(),
        ready_at: crate::clock::now_rfc3339(),
    });
    launch.branch = Some(BranchRecord {
        name: format!("cma/{}", launch.id.0),
        expected_head: launch.source_sha.clone(),
        current_head: launch.source_sha.clone(),
        canonical_url: format!(
            "https://github.com/contributor/project/tree/cma/{}",
            launch.id.0
        ),
        created_at: crate::clock::now_rfc3339(),
    });
    launch.session = Some(SessionLink {
        session_id: session.id.clone(),
        environment_profile_id: session
            .environment_profile
            .as_ref()
            .expect("a Trigger session has a managed profile")
            .0
            .clone(),
        runtime_generation: u64::from(session.rotations) + 1,
        created_at: session.created_at.clone(),
    });
    let now = SystemTime::now();
    let expires = now + Duration::from_secs(300);
    let now_text = crate::clock::rfc3339_at(now);
    launch.reach = Some(ReachStatus {
        service_id: "service-1".into(),
        state: CapabilityRevocationState::Active,
        policy_generation: 1,
        policy_fence: 1,
        runtime_audience: "runtime-generation-1".into(),
        expires_at_epoch_seconds: expires
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        numeric_binding_observed_at: now_text.clone(),
        refreshed_at: now_text.clone(),
        revocation_requested_at: None,
        revocation_effective_at: None,
        last_refusal: None,
        tombstone_acknowledged: false,
        agent_key_revocation_confirmed: false,
    });
    launch.state = LaunchState::PromptPending;
    launch.fencing_counter = 1;
    launch.lease = Some(WorkerLease {
        holder: "prompt-test-worker".into(),
        fencing_token: 1,
        acquired_at: now_text.clone(),
        expires_at: crate::clock::rfc3339_at(expires),
    });
    let claim = EffectClaim {
        kind: LaunchEffectKind::InitialDispatch,
        mode: EffectClaimMode::Execute,
        intent_key: format!("cma-trigger:{}:initial_dispatch:1", launch.id.0),
        attempt: 1,
        reconciliation_attempt: 0,
        fence: 1,
    };
    launch.effects.push(EffectRecord {
        kind: claim.kind,
        state: EffectState::Claimed,
        intent_key: claim.intent_key.clone(),
        attempt: claim.attempt,
        reconciliation_attempt: claim.reconciliation_attempt,
        fence: claim.fence,
        provider_request_id: None,
        external_resource_ids: Default::default(),
        next_attempt_at: None,
        last_failure: None,
        updated_at: now_text,
    });
    (launch, claim)
}

#[test]
fn public_identifiers_are_opaque_and_type_checked() {
    let id = PublicTriggerId::new();
    assert_eq!(PublicTriggerId::parse(id.0.clone()).unwrap(), id);
    assert!(PublicTriggerId::parse("1").is_err());
}

#[test]
fn capability_lifetime_is_hard_bounded() {
    assert_eq!(CAPABILITY_MAX_LIFETIME_SECONDS, 60);
    assert_eq!(CAPABILITY_MAX_CLOCK_SKEW_SECONDS, 5);

    let mut capability = capability();
    capability.validate().unwrap();
    capability.expires_at_epoch_seconds += 1;
    assert!(capability.validate().is_err());
}

#[test]
fn delivery_wire_enum_is_closed_to_seven_values() {
    let values = [
        PromptDeliveryState::Queued,
        PromptDeliveryState::DispatchFenced,
        PromptDeliveryState::WriteConfirmed,
        PromptDeliveryState::ExecutionObserved,
        PromptDeliveryState::Completed,
        PromptDeliveryState::Failed,
        PromptDeliveryState::RecoveryRequired,
    ];
    let encoded: Vec<_> = values
        .into_iter()
        .map(|value| serde_json::to_string(&value).unwrap())
        .collect();
    assert_eq!(encoded.len(), 7);
    assert_eq!(encoded[1], "\"dispatch_fenced\"");
    assert!(serde_json::from_str::<PromptDeliveryState>("\"retrying\"").is_err());
}

#[test]
fn cleanup_dry_run_can_only_name_cma_owned_object_classes() {
    let now = "2026-09-27T12:00:00Z";
    let mut launch = reviewed_launch();
    launch.state = LaunchState::Cancelled;
    launch.terminal_at = Some(NOW.into());
    launch.reach.as_mut().unwrap().state = CapabilityRevocationState::Revoked;
    launch.fencing_counter = 7;
    launch.lease = Some(WorkerLease {
        holder: "cleanup-test".into(),
        fencing_token: 7,
        acquired_at: now.into(),
        expires_at: "2026-09-27T12:02:00Z".into(),
    });
    let lease = launch.lease.clone().unwrap();

    let plan = launch.plan_cleanup(&lease, now, "cleanup-dry-run").unwrap();
    assert_eq!(plan.retention_policy_id, "trigger-test-retention");
    assert_eq!(
        plan.targets,
        vec![
            CleanupTarget::DiffArtifact {
                artifact_id: launch.diff.as_ref().unwrap().id.clone(),
            },
            CleanupTarget::Checkpoint {
                checkpoint_id: launch
                    .workspace
                    .as_ref()
                    .unwrap()
                    .latest_checkpoint
                    .as_ref()
                    .unwrap()
                    .id
                    .clone(),
            },
            CleanupTarget::Baseline,
        ]
    );
    let wire = serde_json::to_string(&plan).unwrap();
    for forbidden in ["sandbox", "fork", "branch", "commit", "pull_request"] {
        assert!(
            !wire.contains(forbidden),
            "cleanup target vocabulary admitted {forbidden}: {wire}"
        );
    }
}

#[test]
fn terminal_cleanup_waits_for_retention_and_capability_revocation_then_is_fenced() {
    let after_retention = "2026-09-27T12:00:00Z";
    let mut launch = reviewed_launch();
    launch.state = LaunchState::Cancelled;
    launch.terminal_at = Some(NOW.into());
    launch.fencing_counter = 11;
    launch.lease = Some(WorkerLease {
        holder: "cleanup-test".into(),
        fencing_token: 11,
        acquired_at: after_retention.into(),
        expires_at: "2026-09-27T12:02:00Z".into(),
    });
    let lease = launch.lease.clone().unwrap();

    assert!(!launch.cleanup_due("2026-09-01T00:00:00Z").unwrap());
    assert!(
        !launch.cleanup_due(after_retention).unwrap(),
        "an active credential must retain its evidence until revocation settles"
    );
    launch.reach.as_mut().unwrap().state = CapabilityRevocationState::Revoked;
    assert!(launch.cleanup_due(after_retention).unwrap());

    let claim = launch
        .claim_next_effect(&lease, after_retention, "cleanup-claim")
        .unwrap()
        .expect("retention creates one fenced cleanup effect");
    assert_eq!(claim.kind, LaunchEffectKind::Cleanup);
    let targets = launch.cleanup.as_ref().unwrap().plan.targets.clone();
    launch
        .complete_effect(
            &lease,
            &claim,
            EffectCompletion {
                result: LaunchEffectResult::Cleanup {
                    receipt: CleanupReceipt {
                        targets: targets.clone(),
                        completed_at: after_retention.into(),
                    },
                },
                safe_detail: "CMA-owned retained objects were deleted.".into(),
                external_resource_ids: BTreeMap::from([(
                    "cleanup_target_count".into(),
                    targets.len().to_string(),
                )]),
                now: after_retention.into(),
                correlation_id: "cleanup-complete".into(),
            },
        )
        .unwrap();
    assert_eq!(launch.state, LaunchState::Cancelled);
    assert_eq!(
        launch.cleanup.as_ref().unwrap().completed_at.as_deref(),
        Some(after_retention)
    );
    assert!(!launch.cleanup_due(after_retention).unwrap());
}

#[test]
fn prompt_delivery_progress_is_identity_bound_monotonic_and_evented() {
    let session_launch = session_launch(&"a".repeat(64));
    let session_id = crate::service::trigger_session_origin(&session_launch)
        .unwrap()
        .session_id()
        .unwrap();
    let session = crate::Session {
        options: Default::default(),
        id: session_id,
        owner: session_launch.owner.clone(),
        kernel: crate::KernelId::new("codex"),
        status: crate::SessionStatus::Ready,
        recovery_state: crate::RecoveryState::Normal,
        stop_reason: None,
        geometry: Default::default(),
        last_seq: 0,
        environment_profile: Some(crate::EnvironmentProfileId("envp-trigger-test".into())),
        runtime_version: Some("trigger-runtime-test".into()),
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        created_at: crate::clock::now_rfc3339(),
    };
    let (mut launch, claim) = prompt_launch(session_launch, &session);
    let lease = launch.lease.clone().unwrap();
    let now = crate::clock::now_rfc3339();
    let mut delivery = prompt_delivery(&launch, PromptDeliveryState::Queued, 0);

    launch
        .observe_effect_progress(
            &lease,
            &claim,
            LaunchEffectResult::InitialDispatch {
                delivery: delivery.clone(),
            },
            now.clone(),
            "queued",
        )
        .unwrap();
    for (state, correlation) in [
        (PromptDeliveryState::DispatchFenced, "fenced"),
        (PromptDeliveryState::WriteConfirmed, "written"),
        (PromptDeliveryState::ExecutionObserved, "observed"),
        (PromptDeliveryState::Completed, "completed"),
    ] {
        delivery.state = state;
        delivery.attempt = 1;
        delivery.dispatch_fence_sequence = Some(42);
        delivery.updated_at = crate::clock::now_rfc3339();
        launch
            .observe_effect_progress(
                &lease,
                &claim,
                LaunchEffectResult::InitialDispatch {
                    delivery: delivery.clone(),
                },
                crate::clock::now_rfc3339(),
                correlation,
            )
            .unwrap();
    }
    assert_eq!(launch.delivery, Some(delivery.clone()));
    assert_eq!(
        launch
            .events
            .iter()
            .filter(|event| matches!(event.kind, LaunchEventKind::DeliveryChanged { .. }))
            .count(),
        5
    );

    for malformed in [
        {
            let mut value = delivery.clone();
            value.origin_key = "another-origin".into();
            value
        },
        {
            let mut value = delivery.clone();
            value.attempt = 0;
            value
        },
        {
            let mut value = delivery.clone();
            value.state = PromptDeliveryState::Queued;
            value.dispatch_fence_sequence = None;
            value
        },
    ] {
        let before = launch.clone();
        assert!(launch
            .observe_effect_progress(
                &lease,
                &claim,
                LaunchEffectResult::InitialDispatch {
                    delivery: malformed,
                },
                now.clone(),
                "illegal-regression",
            )
            .is_err());
        assert_eq!(launch, before, "a rejected observation is atomic");
    }
}

#[test]
fn checkpoint_completion_accepts_only_the_next_artifact_from_the_current_runtime() {
    let mut launch = session_launch(&"a".repeat(64));
    let session = SessionLink {
        session_id: SessionId("sesn_checkpoint_runtime".into()),
        environment_profile_id: "envp_checkpoint_runtime".into(),
        runtime_generation: 3,
        created_at: NOW.into(),
    };
    launch.session = Some(session.clone());
    launch.state = LaunchState::Contributing;
    launch.contribution_checkpoint_version = Some(1);
    launch.fencing_counter = 1;
    let lease = WorkerLease {
        holder: "checkpoint-worker".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.lease = Some(lease.clone());
    let claim = launch
        .claim_next_effect(&lease, NOW, "claim-checkpoint")
        .unwrap()
        .unwrap();
    assert_eq!(claim.kind, LaunchEffectKind::Checkpoint);
    let workspace = launch.workspace.as_ref().unwrap();
    let valid = WorkspaceCheckpoint {
        id: CheckpointId("checkpoint-current-runtime".into()),
        launch_id: launch.id.clone(),
        version: 1,
        workspace_epoch: workspace.workspace_epoch,
        session_id: session.session_id.clone(),
        runtime_generation: session.runtime_generation + 1,
        object_reference: "trigger-test/checkpoint-v1.pack".into(),
        archive_sha256: "b".repeat(64),
        manifest: WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap(),
        integrity: CheckpointIntegrity::Complete,
        created_at: NOW.into(),
        delete_after: Some(workspace.delete_after.clone()),
    };

    for malformed in [
        {
            let mut value = valid.clone();
            value.session_id = SessionId("sesn_other".into());
            value
        },
        {
            let mut value = valid.clone();
            value.runtime_generation = session.runtime_generation - 1;
            value
        },
        {
            let mut value = valid.clone();
            value.version += 1;
            value
        },
        {
            let mut value = valid.clone();
            value.manifest = WorkspaceManifest::seal("c".repeat(40), Vec::new()).unwrap();
            value
        },
        {
            let mut value = valid.clone();
            value.delete_after = Some("2026-08-26T12:00:01Z".into());
            value
        },
    ] {
        let mut candidate = launch.clone();
        assert!(candidate
            .complete_effect(
                &lease,
                &claim,
                EffectCompletion {
                    result: LaunchEffectResult::Checkpoint {
                        checkpoint: malformed,
                    },
                    safe_detail: "must not attach".into(),
                    external_resource_ids: Default::default(),
                    now: LATER.into(),
                    correlation_id: "reject-checkpoint".into(),
                },
            )
            .is_err());
        assert!(candidate
            .workspace
            .as_ref()
            .unwrap()
            .latest_checkpoint
            .is_none());
    }

    launch
        .complete_effect(
            &lease,
            &claim,
            EffectCompletion {
                result: LaunchEffectResult::Checkpoint {
                    checkpoint: valid.clone(),
                },
                safe_detail: "checkpoint complete".into(),
                external_resource_ids: Default::default(),
                now: LATER.into(),
                correlation_id: "complete-checkpoint".into(),
            },
        )
        .unwrap();
    assert_eq!(launch.workspace.unwrap().latest_checkpoint, Some(valid));
    assert_eq!(launch.session.unwrap().runtime_generation, 4);
}

#[test]
fn a_long_initial_turn_uses_its_session_deadline_not_the_generic_poll_budget() {
    let session_launch = session_launch(&"b".repeat(64));
    let session_id = crate::service::trigger_session_origin(&session_launch)
        .unwrap()
        .session_id()
        .unwrap();
    let session = crate::Session {
        options: Default::default(),
        id: session_id,
        owner: session_launch.owner.clone(),
        kernel: crate::KernelId::new("codex"),
        status: crate::SessionStatus::Running,
        recovery_state: crate::RecoveryState::Normal,
        stop_reason: None,
        geometry: Default::default(),
        last_seq: 0,
        environment_profile: Some(crate::EnvironmentProfileId("envp-long-turn".into())),
        runtime_version: Some("trigger-runtime-test".into()),
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        created_at: crate::clock::now_rfc3339(),
    };
    let (mut launch, mut claim) = prompt_launch(session_launch, &session);
    claim.mode = EffectClaimMode::Reconcile;
    claim.reconciliation_attempt = MAX_LAUNCH_RECONCILIATION_ATTEMPTS;
    let effect = launch.effects.last_mut().unwrap();
    effect.state = EffectState::Reconciling;
    effect.reconciliation_attempt = claim.reconciliation_attempt;
    let lease = launch.lease.clone().unwrap();
    let delivery = prompt_delivery(&launch, PromptDeliveryState::WriteConfirmed, 1);
    let now_time = SystemTime::now();
    let now = crate::clock::rfc3339_at(now_time);
    launch
        .observe_effect_progress(
            &lease,
            &claim,
            LaunchEffectResult::InitialDispatch {
                delivery: delivery.clone(),
            },
            now.clone(),
            "observe-long-turn",
        )
        .unwrap();
    let transient = LaunchFailure {
        class: FailureClass::RetryableTransient,
        code: "initial_prompt_turn_pending".into(),
        detail: "The initial task is still running.".into(),
        remediation: "Continue observing the same session turn.".into(),
        correlation_id: "long-turn".into(),
        retry_at: None,
        observed_at: now.clone(),
    };
    launch
        .defer_reconciliation(
            &lease,
            &claim,
            transient.clone(),
            crate::clock::rfc3339_at(now_time + Duration::from_secs(30)),
            Default::default(),
            now.clone(),
        )
        .expect("poll count alone never abandons a live initial turn");
    assert_eq!(launch.state, LaunchState::PromptPending);
    assert_eq!(
        launch.effects.last().unwrap().state,
        EffectState::Reconciling
    );

    launch.delivery.as_mut().unwrap().queued_at = "2020-01-01T00:00:00Z".into();
    launch
        .defer_reconciliation(
            &lease,
            &claim,
            transient,
            crate::clock::rfc3339_at(now_time + Duration::from_secs(30)),
            Default::default(),
            now,
        )
        .expect("the wall-clock deadline resolves to explicit recovery");
    assert_eq!(launch.state, LaunchState::FailedRetryable);
    assert_eq!(
        launch.blocked_reason,
        Some(LaunchBlockedReason::RecoveryRequired)
    );
    assert_eq!(
        launch.effects.last().unwrap().state,
        EffectState::RecoveryRequired
    );
}

#[test]
fn prompt_recovery_is_payload_bound_explicit_and_one_use() {
    let mut launch = recovery_required_prompt_launch();
    let before = launch.clone();
    let mut confirmation = confirmation(
        ConfirmationAction::RecoverPrompt,
        &launch.owner,
        &launch.delivery.as_ref().unwrap().payload_hash,
    );
    confirmation.idempotency_key_hash = "recover-prompt-one".into();

    assert!(launch
        .authorize_initial_prompt_recovery(confirmation.clone(), false, LATER, "missing-risk-ack",)
        .is_err());
    assert_eq!(launch, before, "a refused recovery must be atomic");

    let mut wrong_payload = confirmation.clone();
    wrong_payload.snapshot_digest = "b".repeat(64);
    assert!(launch
        .authorize_initial_prompt_recovery(wrong_payload, true, LATER, "wrong-payload")
        .is_err());
    assert_eq!(
        launch, before,
        "a mismatched payload must not reopen delivery"
    );

    let intent = launch.effects.last().unwrap().intent_key.clone();
    let attempt = launch.effects.last().unwrap().attempt;
    launch
        .authorize_initial_prompt_recovery(confirmation.clone(), true, LATER, "recover-prompt")
        .unwrap();
    assert_eq!(launch.state, LaunchState::PromptPending);
    assert_eq!(launch.blocked_reason, None);
    assert_eq!(launch.failure, None);
    assert_eq!(launch.resume_state, None);
    assert_eq!(launch.effects.last().unwrap().state, EffectState::Queued);
    assert_eq!(launch.effects.last().unwrap().intent_key, intent);
    assert_eq!(launch.effects.last().unwrap().attempt, attempt);
    assert_eq!(
        launch.delivery.as_ref().unwrap().state,
        PromptDeliveryState::RecoveryRequired,
        "the session outbox, not the browser command, remains final send authority"
    );
    assert!(launch.events.iter().any(|event| matches!(
        event.kind,
        LaunchEventKind::RecoveryAuthorized {
            effect: LaunchEffectKind::InitialDispatch
        }
    )));

    launch.state = LaunchState::FailedRetryable;
    launch.blocked_reason = Some(LaunchBlockedReason::RecoveryRequired);
    launch.resume_state = Some(LaunchState::PromptPending);
    launch.failure = Some(recovery_failure("second-ambiguity"));
    launch.effects.last_mut().unwrap().state = EffectState::RecoveryRequired;
    launch.delivery.as_mut().unwrap().recovery_generation = 1;
    launch.delivery.as_mut().unwrap().attempt += 1;
    let before_reuse = launch.clone();
    assert!(launch
        .authorize_initial_prompt_recovery(confirmation, true, LATER, "reuse-confirmation",)
        .is_err());
    assert_eq!(
        launch, before_reuse,
        "one consent cannot authorize two sends"
    );
}

#[test]
fn operator_recovery_can_only_schedule_read_only_reconciliation() {
    let (mut launch, lease) = leased_started_launch();
    let claim = launch
        .claim_next_effect(&lease, NOW, "initial-claim")
        .unwrap()
        .unwrap();
    launch.effects.last_mut().unwrap().state = EffectState::RecoveryRequired;
    launch.effects.last_mut().unwrap().last_failure = Some(recovery_failure("operator-evidence"));
    launch.lease = None;
    launch.state = LaunchState::FailedRetryable;
    launch.blocked_reason = Some(LaunchBlockedReason::RecoveryRequired);
    launch.resume_state = Some(LaunchState::Preflight);
    launch.failure = Some(recovery_failure("operator-evidence"));

    for (intent, fence, evidence) in [
        ("wrong-intent", claim.fence, "operator-evidence"),
        (&claim.intent_key, claim.fence + 1, "operator-evidence"),
        (&claim.intent_key, claim.fence, "wrong-evidence"),
    ] {
        let mut candidate = launch.clone();
        assert!(candidate
            .authorize_effect_reconciliation(
                claim.kind,
                intent,
                fence,
                evidence,
                LATER,
                "operator-authorize",
            )
            .is_err());
        assert_eq!(candidate, launch, "a mismatched recovery request is atomic");
    }

    let mut mismatched_failure = launch.clone();
    mismatched_failure.effects.last_mut().unwrap().last_failure =
        Some(recovery_failure("another-effect-failure"));
    let before = mismatched_failure.clone();
    assert!(mismatched_failure
        .authorize_effect_reconciliation(
            claim.kind,
            &claim.intent_key,
            claim.fence,
            "operator-evidence",
            LATER,
            "operator-authorize",
        )
        .is_err());
    assert_eq!(mismatched_failure, before);

    let mut wrong_step = launch.clone();
    wrong_step.effects.push(EffectRecord {
        kind: LaunchEffectKind::Fork,
        state: EffectState::RecoveryRequired,
        intent_key: "wrong-step-intent".into(),
        attempt: 1,
        reconciliation_attempt: 1,
        fence: claim.fence,
        provider_request_id: None,
        external_resource_ids: BTreeMap::new(),
        next_attempt_at: None,
        last_failure: Some(recovery_failure("operator-evidence")),
        updated_at: NOW.into(),
    });
    let before = wrong_step.clone();
    assert!(wrong_step
        .authorize_effect_reconciliation(
            LaunchEffectKind::Fork,
            "wrong-step-intent",
            claim.fence,
            "operator-evidence",
            LATER,
            "operator-authorize",
        )
        .is_err());
    assert_eq!(wrong_step, before);

    launch
        .authorize_effect_reconciliation(
            claim.kind,
            &claim.intent_key,
            claim.fence,
            "operator-evidence",
            LATER,
            "operator-authorize",
        )
        .unwrap();
    assert_eq!(launch.state, LaunchState::Preflight);
    assert_eq!(
        launch.effects.last().unwrap().state,
        EffectState::Reconciling
    );
    assert_eq!(
        launch.effects.last().unwrap().next_attempt_at.as_deref(),
        Some(LATER)
    );

    let replacement = WorkerLease {
        holder: "operator-reconcile-worker".into(),
        fencing_token: launch.fencing_counter + 1,
        acquired_at: LATER.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = replacement.fencing_token;
    launch.lease = Some(replacement.clone());
    let recovered = launch
        .claim_next_effect(&replacement, LATER, "reconcile-after-authorization")
        .unwrap()
        .unwrap();
    assert_eq!(recovered.mode, EffectClaimMode::Reconcile);
    assert_eq!(recovered.intent_key, claim.intent_key);
    assert_eq!(recovered.attempt, claim.attempt);
}

fn recovery_required_prompt_launch() -> Launch {
    let session_launch = session_launch(&"d".repeat(64));
    let session_id = crate::service::trigger_session_origin(&session_launch)
        .unwrap()
        .session_id()
        .unwrap();
    let session = crate::Session {
        options: Default::default(),
        id: session_id,
        owner: session_launch.owner.clone(),
        kernel: crate::KernelId::new("codex"),
        status: crate::SessionStatus::Running,
        recovery_state: crate::RecoveryState::Normal,
        stop_reason: None,
        geometry: Default::default(),
        last_seq: 0,
        environment_profile: Some(crate::EnvironmentProfileId("envp-recovery-test".into())),
        runtime_version: Some("trigger-runtime-test".into()),
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        created_at: NOW.into(),
    };
    let (mut launch, _) = prompt_launch(session_launch, &session);
    launch.delivery = Some(prompt_delivery(
        &launch,
        PromptDeliveryState::RecoveryRequired,
        1,
    ));
    launch.effects.last_mut().unwrap().state = EffectState::RecoveryRequired;
    launch.effects.last_mut().unwrap().last_failure = Some(recovery_failure("prompt-ambiguity"));
    launch.lease = None;
    launch.state = LaunchState::FailedRetryable;
    launch.blocked_reason = Some(LaunchBlockedReason::RecoveryRequired);
    launch.resume_state = Some(LaunchState::PromptPending);
    launch.failure = Some(recovery_failure("prompt-ambiguity"));
    launch
}

fn recovery_failure(correlation_id: &str) -> LaunchFailure {
    LaunchFailure {
        class: FailureClass::RecoveryRequired,
        code: "effect_outcome_unknown".into(),
        detail: "CMA cannot prove the external outcome.".into(),
        remediation: "Review retained evidence before authorizing recovery.".into(),
        correlation_id: correlation_id.into(),
        retry_at: None,
        observed_at: NOW.into(),
    }
}

fn prompt_delivery(launch: &Launch, state: PromptDeliveryState, attempt: u32) -> PromptDelivery {
    PromptDelivery {
        launch_id: launch.id.clone(),
        session_id: launch.session.as_ref().unwrap().session_id.clone(),
        revision_digest: launch.revision_digest.clone(),
        origin_key: format!(
            "cma-trigger:{}:{}:initial",
            launch.id.0, launch.revision_digest
        ),
        payload_hash: "a".repeat(64),
        state,
        attempt,
        recovery_generation: 0,
        dispatch_fence_sequence: (state != PromptDeliveryState::Queued).then_some(42),
        queued_at: crate::clock::now_rfc3339(),
        updated_at: crate::clock::now_rfc3339(),
    }
}

#[test]
fn branch_publication_requires_the_exact_valid_review_and_records_intent_first() {
    let mut invalid = reviewed_launch();
    invalid.diff.as_mut().unwrap().validation = DiffValidationOutcome::NoChanges;
    let invalid_confirmation = confirmation(
        ConfirmationAction::PublishBranch,
        &invalid.owner,
        &invalid.diff.as_ref().unwrap().digest,
    );
    assert!(invalid
        .confirm_branch_publication(invalid_confirmation, NOW, "invalid-review")
        .is_err());
    assert_eq!(invalid.state, LaunchState::Contributing);
    assert!(invalid.contribution.is_none());

    let mut launch = reviewed_launch();
    let wrong = confirmation(
        ConfirmationAction::PublishBranch,
        &launch.owner,
        &"e".repeat(64),
    );
    assert!(launch
        .confirm_branch_publication(wrong, NOW, "wrong-snapshot")
        .is_err());
    assert!(launch.contribution.is_none());

    let diff = launch.diff.clone().unwrap();
    launch
        .confirm_branch_publication(
            confirmation(
                ConfirmationAction::PublishBranch,
                &launch.owner,
                &diff.digest,
            ),
            NOW,
            "confirm-publication",
        )
        .unwrap();
    let intent = launch.contribution.as_ref().unwrap();
    assert_eq!(launch.state, LaunchState::PrPending);
    assert_eq!(intent.state, PublicationState::IntentRecorded);
    assert_eq!(intent.diff_artifact_id, diff.id);
    assert_eq!(intent.diff_digest, diff.digest);
    assert_eq!(intent.checkpoint_digest, diff.checkpoint_digest);
    assert_eq!(intent.expected_head, diff.expected_branch_head);
    assert!(intent.commit_id.is_none());

    let lease = WorkerLease {
        holder: "publication-worker".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = 1;
    launch.lease = Some(lease.clone());
    let claim = launch
        .claim_next_effect(&lease, NOW, "claim-publication")
        .unwrap()
        .unwrap();
    assert_eq!(claim.kind, LaunchEffectKind::BranchPublication);
    assert!(launch.contribution.is_some(), "intent precedes the claim");
}

#[test]
fn publication_progress_is_monotonic_and_does_not_authorize_a_pull_request() {
    let mut launch = reviewed_launch();
    let diff_digest = launch.diff.as_ref().unwrap().digest.clone();
    launch
        .confirm_branch_publication(
            confirmation(
                ConfirmationAction::PublishBranch,
                &launch.owner,
                &diff_digest,
            ),
            NOW,
            "confirm-publication",
        )
        .unwrap();
    let lease = WorkerLease {
        holder: "publication-worker".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = 1;
    launch.lease = Some(lease.clone());
    let claim = launch
        .claim_next_effect(&lease, NOW, "claim-publication")
        .unwrap()
        .unwrap();
    let mut objects = launch.contribution.clone().unwrap();
    objects.state = PublicationState::ObjectsWritten;
    objects.base_tree_id = Some("b".repeat(40));
    objects.tree_id = Some("c".repeat(40));
    objects.commit_id = Some("d".repeat(40));
    objects.object_ids = vec!["e".repeat(40)];
    objects.updated_at = LATER.into();
    launch
        .observe_effect_progress(
            &lease,
            &claim,
            LaunchEffectResult::BranchPublication {
                contribution: objects.clone(),
            },
            LATER,
            "objects-written",
        )
        .unwrap();
    assert_eq!(
        launch.contribution.as_ref().unwrap().state,
        PublicationState::ObjectsWritten
    );

    let mut changed_objects = objects.clone();
    changed_objects.state = PublicationState::RefUpdatePending;
    changed_objects.commit_id = Some("f".repeat(40));
    let before = launch.clone();
    assert!(launch
        .observe_effect_progress(
            &lease,
            &claim,
            LaunchEffectResult::BranchPublication {
                contribution: changed_objects,
            },
            LATER,
            "changed-evidence",
        )
        .is_err());
    assert_eq!(launch, before, "rejected progress is atomic");

    let mut pending = objects;
    pending.state = PublicationState::RefUpdatePending;
    launch
        .observe_effect_progress(
            &lease,
            &claim,
            LaunchEffectResult::BranchPublication {
                contribution: pending.clone(),
            },
            LATER,
            "ref-pending",
        )
        .unwrap();
    let mut published = pending;
    published.state = PublicationState::Published;
    published.resulting_head = published.commit_id.clone();
    let mut replaced_intent = published.clone();
    replaced_intent.id = ContributionId::new();
    let before = launch.clone();
    assert!(launch
        .complete_effect(
            &lease,
            &claim,
            EffectCompletion {
                result: LaunchEffectResult::BranchPublication {
                    contribution: replaced_intent,
                },
                safe_detail: "branch published".into(),
                external_resource_ids: BTreeMap::from([
                    ("base_tree_id".into(), "b".repeat(40)),
                    ("tree_id".into(), "c".repeat(40)),
                    ("commit_id".into(), "d".repeat(40)),
                    ("resulting_head".into(), "d".repeat(40)),
                ]),
                now: LATER.into(),
                correlation_id: "replaced-publication-intent".into(),
            },
        )
        .is_err());
    assert_eq!(launch, before, "publication completion is intent-bound");
    launch
        .complete_effect(
            &lease,
            &claim,
            EffectCompletion {
                result: LaunchEffectResult::BranchPublication {
                    contribution: published.clone(),
                },
                safe_detail: "branch published".into(),
                external_resource_ids: BTreeMap::from([
                    ("base_tree_id".into(), "b".repeat(40)),
                    ("tree_id".into(), "c".repeat(40)),
                    ("commit_id".into(), "d".repeat(40)),
                    ("resulting_head".into(), "d".repeat(40)),
                ]),
                now: LATER.into(),
                correlation_id: "publication-complete".into(),
            },
        )
        .unwrap();
    assert_eq!(launch.contribution, Some(published));
    assert_eq!(launch.branch.as_ref().unwrap().current_head, "d".repeat(40));
    assert!(launch.pull_request.is_none());

    launch.lease = None;
    launch.fencing_counter = 2;
    let next_lease = WorkerLease {
        holder: "next-worker".into(),
        fencing_token: 2,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.lease = Some(next_lease.clone());
    assert!(launch
        .claim_next_effect(&next_lease, LATER, "no-implicit-pr")
        .unwrap()
        .is_none());
}

#[test]
fn pull_request_confirmation_seals_cross_fork_identity_before_the_worker_claim() {
    let mut launch = published_contribution_launch();
    let draft = launch
        .pull_request_draft(PullRequestOperation::Open, None, None)
        .expect("build inert PR review");
    assert_eq!(draft.marker, pull_request_marker(&launch.id));
    assert_eq!(draft.head_owner, "contributor");
    assert_eq!(draft.head_branch, format!("cma/{}", launch.id.0));
    assert_eq!(draft.head_ref, format!("contributor:cma/{}", launch.id.0));
    assert_eq!(draft.upstream_repository_id, 10);
    assert_eq!(draft.fork_repository_id, 11);
    assert_eq!(draft.expected_head_revision, "d".repeat(40));
    assert!(draft.provider_body().ends_with(&draft.marker));

    let mut confirmation = confirmation(
        ConfirmationAction::OpenPullRequest,
        &launch.owner,
        &draft.intent_digest,
    );
    confirmation.idempotency_key_hash = "open-pr-one".into();
    launch
        .confirm_pull_request(confirmation.clone(), None, None, NOW, "confirm-pr")
        .expect("record open intent");
    let record = launch.pull_request.as_ref().unwrap();
    assert_eq!(record.state, PullRequestState::Creating);
    assert_eq!(record.confirmation, confirmation);
    assert!(record.number.is_none());

    let confirmation_count = launch.confirmations.len();
    let mut repeated = confirmation.clone();
    repeated.idempotency_key_hash = "open-pr-second-tab".into();
    launch
        .confirm_pull_request(repeated, None, None, LATER, "repeat-pr")
        .expect("same sealed intent is inert");
    assert_eq!(launch.confirmations.len(), confirmation_count);

    let lease = WorkerLease {
        holder: "pr-worker".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = 1;
    launch.lease = Some(lease.clone());
    let claim = launch
        .claim_next_effect(&lease, NOW, "claim-pr")
        .unwrap()
        .unwrap();
    assert_eq!(claim.kind, LaunchEffectKind::PullRequest);
    assert!(launch.pull_request.is_some(), "intent precedes the claim");
}

#[test]
fn pull_request_completion_cannot_replace_the_retained_intent() {
    let mut launch = published_contribution_launch();
    let draft = launch
        .pull_request_draft(PullRequestOperation::Open, None, None)
        .unwrap();
    launch
        .confirm_pull_request(
            confirmation(
                ConfirmationAction::OpenPullRequest,
                &launch.owner,
                &draft.intent_digest,
            ),
            None,
            None,
            NOW,
            "confirm-pr",
        )
        .unwrap();
    let lease = WorkerLease {
        holder: "pr-worker".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = 1;
    launch.lease = Some(lease.clone());
    let claim = launch
        .claim_next_effect(&lease, NOW, "claim-pr")
        .unwrap()
        .unwrap();
    let reconciled = reconciled_pull_request(launch.pull_request.clone().unwrap());

    let mut replaced = reconciled.clone();
    replaced.intent = PullRequestDraft::seal(
        launch.id.clone(),
        PullRequestOperation::Open,
        10,
        "R_upstream".into(),
        11,
        "R_fork".into(),
        "main".into(),
        "contributor".into(),
        format!("cma/{}", launch.id.0),
        "a".repeat(40),
        "d".repeat(40),
        None,
        None,
        None,
        None,
        false,
        None,
        None,
        "Different reviewed title".into(),
        "Different reviewed body".into(),
    )
    .unwrap();
    replaced.confirmation.snapshot_digest = replaced.intent.intent_digest.clone();
    replaced.intent_key = format!(
        "cma-trigger:{}:pr:open:{}",
        launch.id.0, replaced.intent.intent_digest
    );
    let before = launch.clone();
    assert!(launch
        .complete_effect(
            &lease,
            &claim,
            EffectCompletion {
                result: LaunchEffectResult::PullRequest {
                    pull_request: replaced,
                },
                safe_detail: "PR ready".into(),
                external_resource_ids: pull_request_resources(),
                now: LATER.into(),
                correlation_id: "replaced-pr-intent".into(),
            },
        )
        .is_err());
    assert_eq!(launch, before, "rejected completion is atomic");

    launch
        .complete_effect(
            &lease,
            &claim,
            EffectCompletion {
                result: LaunchEffectResult::PullRequest {
                    pull_request: reconciled,
                },
                safe_detail: "PR ready".into(),
                external_resource_ids: pull_request_resources(),
                now: LATER.into(),
                correlation_id: "complete-pr".into(),
            },
        )
        .unwrap();
    assert_eq!(launch.state, LaunchState::PrOpen);
    assert_eq!(launch.pull_request.as_ref().unwrap().number, Some(7));
}

#[test]
fn pull_request_update_requires_a_new_reviewed_commit_and_preserves_provider_identity() {
    let mut launch = published_contribution_launch();
    let draft = launch
        .pull_request_draft(PullRequestOperation::Open, None, None)
        .unwrap();
    launch
        .confirm_pull_request(
            confirmation(
                ConfirmationAction::OpenPullRequest,
                &launch.owner,
                &draft.intent_digest,
            ),
            None,
            None,
            NOW,
            "confirm-open",
        )
        .unwrap();
    launch.pull_request = Some(reconciled_pull_request(launch.pull_request.take().unwrap()));
    launch.state = LaunchState::PrPending;
    let contribution = launch.contribution.as_mut().unwrap();
    contribution.expected_head = "d".repeat(40);
    contribution.commit_id = Some("e".repeat(40));
    contribution.resulting_head = Some("e".repeat(40));
    launch.branch.as_mut().unwrap().current_head = "e".repeat(40);

    let update = launch
        .pull_request_draft(PullRequestOperation::Update, None, None)
        .expect("build update review");
    assert_eq!(update.previous_head_revision, Some("d".repeat(40)));
    assert_eq!(update.expected_head_revision, "e".repeat(40));
    assert_eq!(update.target_number, Some(7));
    assert_eq!(update.target_provider_id, Some(70));
    assert_eq!(update.target_provider_node_id.as_deref(), Some("PR_node_7"));
    assert!(!update.update_metadata);
    assert!(update.expected_previous_title.is_none());
    assert!(update.expected_previous_body_digest.is_none());

    launch
        .confirm_pull_request(
            confirmation(
                ConfirmationAction::UpdatePullRequest,
                &launch.owner,
                &update.intent_digest,
            ),
            None,
            None,
            LATER,
            "confirm-update",
        )
        .unwrap();
    assert_eq!(
        launch.pull_request.as_ref().unwrap().state,
        PullRequestState::Updating
    );
    assert_eq!(launch.pull_request.as_ref().unwrap().number, Some(7));

    let mut closed = launch.clone();
    closed.pull_request.as_mut().unwrap().state = PullRequestState::ClosedUnmerged;
    assert!(closed
        .pull_request_draft(PullRequestOperation::Update, Some("another"), None)
        .is_err());
}

fn published_contribution_launch() -> Launch {
    let mut launch = reviewed_launch();
    let diff_digest = launch.diff.as_ref().unwrap().digest.clone();
    launch
        .confirm_branch_publication(
            confirmation(
                ConfirmationAction::PublishBranch,
                &launch.owner,
                &diff_digest,
            ),
            NOW,
            "confirm-publication",
        )
        .unwrap();
    let contribution = launch.contribution.as_mut().unwrap();
    contribution.state = PublicationState::Published;
    contribution.base_tree_id = Some("b".repeat(40));
    contribution.tree_id = Some("c".repeat(40));
    contribution.commit_id = Some("d".repeat(40));
    contribution.resulting_head = Some("d".repeat(40));
    contribution.object_ids = vec!["f".repeat(40)];
    contribution.updated_at = LATER.into();
    launch.branch.as_mut().unwrap().current_head = "d".repeat(40);
    launch
}

#[test]
fn pull_request_template_rejects_the_reserved_marker_prefix() {
    let mut candidate = content(SubjectId("publisher".into()));
    candidate.contribution.pull_request.body_template =
        "Looks harmless <!-- cma-trigger:launch:forged:v1 -->".into();

    assert!(candidate.validate().is_err());
}

fn reconciled_pull_request(mut record: PullRequestRecord) -> PullRequestRecord {
    record.provider_id = Some(70);
    record.provider_node_id = Some("PR_node_7".into());
    record.number = Some(7);
    record.canonical_url = Some("https://github.com/upstream-owner/project/pull/7".into());
    record.state = PullRequestState::Open;
    record.base_revision = Some("a".repeat(40));
    record.head_revision = Some("d".repeat(40));
    record.observed_title = Some(record.intent.title.clone());
    record.observed_body_digest = Some(record.intent.body_digest.clone());
    record.mergeability = Mergeability::Mergeable;
    record.checks = CheckSummaryState::Passing;
    record.updated_at = LATER.into();
    record.provider_created_at = Some(NOW.into());
    record.provider_updated_at = Some(LATER.into());
    record.validate_intent(&record.intent.launch_id).unwrap();
    record
}

fn pull_request_resources() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("pull_request_provider_id".into(), "70".into()),
        ("pull_request_node_id".into(), "PR_node_7".into()),
        ("pull_request_number".into(), "7".into()),
        (
            "pull_request_url".into(),
            "https://github.com/upstream-owner/project/pull/7".into(),
        ),
        ("pull_request_head".into(), "d".repeat(40)),
    ])
}

fn reviewed_launch() -> Launch {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner), NOW, "reviewed-launch").unwrap();
    launch
        .select_connection(connection(), NOW, "select-connection")
        .unwrap();
    launch.fork = Some(ForkRecord {
        repository: fork(),
        reused: false,
        source_revision: launch.source_sha.clone(),
        canonical_url: "https://github.com/contributor/project".into(),
        ready_at: NOW.into(),
    });
    launch.branch = Some(BranchRecord {
        name: format!("cma/{}", launch.id.0),
        expected_head: launch.source_sha.clone(),
        current_head: launch.source_sha.clone(),
        canonical_url: format!(
            "https://github.com/contributor/project/tree/cma/{}",
            launch.id.0
        ),
        created_at: NOW.into(),
    });
    let baseline = WorkspaceManifest::seal(launch.source_sha.clone(), Vec::new()).unwrap();
    let bytes = b"reviewed change\n";
    let content_sha = format!("{:x}", sha2::Sha256::digest(bytes));
    let checkpoint_manifest = WorkspaceManifest::seal(
        launch.source_sha.clone(),
        vec![WorkspaceEntry {
            path: "README.md".into(),
            kind: WorkspaceEntryKind::Regular,
            mode: 0o100644,
            size: bytes.len() as u64,
            content_sha256: content_sha.clone(),
            provider_object_id: None,
            symlink_target: None,
        }],
    )
    .unwrap();
    let checkpoint = WorkspaceCheckpoint {
        id: CheckpointId::new(),
        launch_id: launch.id.clone(),
        version: 1,
        workspace_epoch: 1,
        session_id: SessionId("sesn_publication".into()),
        runtime_generation: 1,
        object_reference: "trigger-test/checkpoint.pack".into(),
        archive_sha256: "a".repeat(64),
        manifest: checkpoint_manifest,
        integrity: CheckpointIntegrity::Complete,
        created_at: NOW.into(),
        delete_after: Some("2026-09-26T12:00:00Z".into()),
    };
    launch.workspace = Some(WorkspaceRecord {
        baseline: baseline.clone(),
        baseline_object_reference: "trigger-test/baseline.pack".into(),
        baseline_archive_sha256: "b".repeat(64),
        latest_checkpoint: Some(checkpoint.clone()),
        previous_checkpoint: None,
        workspace_epoch: 1,
        retention_policy_id: "trigger-test-retention".into(),
        delete_after: "2026-09-26T12:00:00Z".into(),
    });
    let session_id = SessionId("sesn_publication".into());
    launch.session = Some(SessionLink {
        session_id: session_id.clone(),
        environment_profile_id: "envp_publication".into(),
        runtime_generation: 1,
        created_at: NOW.into(),
    });
    launch.reach = Some(ReachStatus {
        service_id: "service-1".into(),
        state: CapabilityRevocationState::Active,
        policy_generation: 1,
        policy_fence: 1,
        runtime_audience: "runtime-publication".into(),
        expires_at_epoch_seconds: 4_000_000_000,
        numeric_binding_observed_at: NOW.into(),
        refreshed_at: NOW.into(),
        revocation_requested_at: None,
        revocation_effective_at: None,
        last_refusal: None,
        tombstone_acknowledged: false,
        agent_key_revocation_confirmed: false,
    });
    launch.delivery = Some(PromptDelivery {
        launch_id: launch.id.clone(),
        session_id,
        revision_digest: launch.revision_digest.clone(),
        origin_key: format!("cma-trigger:{}:initial", launch.id.0),
        payload_hash: "f".repeat(64),
        state: PromptDeliveryState::Completed,
        attempt: 1,
        recovery_generation: 0,
        dispatch_fence_sequence: Some(1),
        queued_at: NOW.into(),
        updated_at: NOW.into(),
    });
    launch.diff = Some(DiffArtifact {
        id: DiffArtifactId::new(),
        launch_id: launch.id.clone(),
        checkpoint_id: checkpoint.id,
        baseline_digest: baseline.digest,
        checkpoint_digest: checkpoint.manifest.digest,
        expected_branch_head: launch.source_sha.clone(),
        observed_branch_head: launch.source_sha.clone(),
        changed_file_limit: 1_000,
        total_changed_files: 1,
        files: vec![DiffFile {
            path: "README.md".into(),
            change: DiffChangeKind::Added,
            binary: false,
            old_mode: None,
            new_mode: Some(0o100644),
            old_sha256: None,
            new_sha256: Some(content_sha),
            old_provider_object_id: None,
            new_provider_object_id: None,
            old_bytes: 0,
            new_bytes: bytes.len() as u64,
            patch: Some("--- baseline\n+++ checkpoint\n@@ -0,0 +1 @@\n+reviewed change\n".into()),
            patch_complete: true,
        }],
        changed_bytes: bytes.len() as u64,
        digest: "c".repeat(64),
        object_reference: "trigger-test/diff.json".into(),
        object_sha256: "c".repeat(64),
        object_bytes: 1,
        author: CommitIdentity {
            name: "contributor".into(),
            email: "21+contributor@users.noreply.github.com".into(),
        },
        committer: CommitIdentity {
            name: "CMA".into(),
            email: "cma@chrono.ai".into(),
        },
        workflow_paths: Vec::new(),
        validation: DiffValidationOutcome::Valid,
        checks: Vec::new(),
        created_at: NOW.into(),
        delete_after: "2026-09-26T12:00:00Z".into(),
    });
    launch.state = LaunchState::Contributing;
    launch
        .diff
        .as_ref()
        .unwrap()
        .validate_for_publication(&launch)
        .unwrap();
    launch
}

#[test]
fn preview_is_an_allowlist_and_never_projects_the_prompt() {
    let revision = revision(SubjectId("publisher".into()));
    let preview = TriggerPreview::from(&revision);
    let wire = serde_json::to_string(&preview).unwrap();
    assert!(wire.contains("Fix one bounded issue"));
    assert!(!wire.contains("Implement {{topic}}"));
    assert!(!wire.contains("internal-pricing-policy"));
}

#[test]
fn locked_overrides_and_cross_dimension_resource_ceilings_fail_closed() {
    let mut revision = revision(SubjectId("publisher".into()));
    let locked = LaunchOverrides {
        branch_suffix: Some("mine".into()),
        ..LaunchOverrides::default()
    };
    assert!(effective_configuration(&revision, &locked).is_err());

    revision
        .content
        .override_policy
        .editable_fields
        .push(EditableField::ResourceSize);
    revision.digest = revision.content.digest().unwrap();
    let too_much_memory = LaunchOverrides {
        resource_size: Some(SessionSize::new(500, 4_096).unwrap()),
        ..LaunchOverrides::default()
    };
    assert!(effective_configuration(&revision, &too_much_memory).is_err());
}

#[test]
fn authentication_recovery_returns_to_the_recorded_durable_step() {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner.clone()), NOW, "create").unwrap();
    launch
        .select_connection(connection(), NOW, "connection")
        .unwrap();
    launch
        .start(
            confirmation(ConfirmationAction::Start, &owner, &launch.effective.digest),
            NOW,
            "start",
        )
        .unwrap();
    assert_eq!(launch.state, LaunchState::Preflight);

    launch
        .fail(
            LaunchFailure {
                class: FailureClass::AuthRequired,
                code: "github_reauthorization_required".into(),
                detail: "The selected connection no longer authorizes this repository.".into(),
                remediation: "Reconnect the same GitHub account.".into(),
                correlation_id: "auth-failure".into(),
                retry_at: None,
                observed_at: LATER.into(),
            },
            LATER,
        )
        .unwrap();
    assert_eq!(launch.state, LaunchState::AuthRequired);
    assert_eq!(launch.resume_state, Some(LaunchState::Preflight));

    launch
        .select_connection(connection(), LATER, "reauthorized")
        .unwrap();
    assert_eq!(launch.state, LaunchState::Preflight);
    assert_eq!(launch.resume_state, None);
    assert_eq!(launch.failure, None);
}

#[test]
fn workflow_paths_require_an_explicit_reviewed_confirmation_before_start() {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner.clone()), NOW, "create").unwrap();
    launch
        .select_connection(connection(), NOW, "connection")
        .unwrap();
    launch.workflow_paths = vec![".github/workflows/checks.yml".into()];
    let start = confirmation(ConfirmationAction::Start, &owner, &launch.effective.digest);

    assert!(launch
        .start(start.clone(), NOW, "start-without-risk")
        .is_err());
    assert_eq!(launch.state, LaunchState::Draft);

    launch
        .acknowledge_workflow_risk(
            confirmation(
                ConfirmationAction::WorkflowRisk,
                &owner,
                &launch.effective.digest,
            ),
            "workflow-risk",
        )
        .unwrap();
    launch.start(start, NOW, "start").unwrap();
    assert_eq!(launch.state, LaunchState::Preflight);
    assert!(launch.workflow_risk_acknowledged);
    assert!(launch
        .confirmations
        .iter()
        .any(|confirmation| { confirmation.action == ConfirmationAction::WorkflowRisk }));
}

#[test]
fn cancellation_is_idempotent_but_never_skips_ambiguous_pr_reconciliation() {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner.clone()), NOW, "create").unwrap();
    launch.state = LaunchState::PrPending;
    launch.effects.push(EffectRecord {
        kind: LaunchEffectKind::PullRequest,
        state: EffectState::Reconciling,
        intent_key: "pr-intent".into(),
        attempt: 1,
        reconciliation_attempt: 1,
        fence: 1,
        provider_request_id: None,
        external_resource_ids: Default::default(),
        next_attempt_at: None,
        last_failure: None,
        updated_at: NOW.into(),
    });
    let cancel = confirmation(ConfirmationAction::Cancel, &owner, &launch.effective.digest);
    assert!(launch.cancel(cancel.clone(), LATER, "cancel").is_err());
    assert_eq!(launch.state, LaunchState::PrPending);

    launch.effects.clear();
    launch.cancel(cancel.clone(), LATER, "cancel").unwrap();
    assert_eq!(launch.state, LaunchState::Cancelled);
    launch.cancel(cancel, LATER, "cancel-again").unwrap();
    assert_eq!(launch.state, LaunchState::Cancelled);
}

#[test]
fn cancellation_settles_pending_work_and_preserves_every_external_record() {
    use LaunchState as State;

    for state in [
        State::Draft,
        State::AuthRequired,
        State::Preflight,
        State::ForkPending,
        State::ForkReady,
        State::BranchPending,
        State::WorkspacePending,
        State::SandboxPending,
        State::SessionPending,
        State::PromptPending,
        State::Ready,
        State::Contributing,
        State::PrPending,
        State::PrOpen,
        State::FailedRetryable,
    ] {
        let mut launch = reviewed_launch();
        launch.state = state;
        launch.blocked_reason = None;
        launch.failure = None;
        launch.resume_state = None;
        launch.effects = vec![
            EffectRecord {
                kind: LaunchEffectKind::Fork,
                state: EffectState::Succeeded,
                intent_key: "completed-fork".into(),
                attempt: 1,
                reconciliation_attempt: 0,
                fence: 3,
                provider_request_id: Some("provider-fork".into()),
                external_resource_ids: BTreeMap::from([("fork_repository_id".into(), "20".into())]),
                next_attempt_at: None,
                last_failure: None,
                updated_at: NOW.into(),
            },
            EffectRecord {
                kind: LaunchEffectKind::Checkpoint,
                state: EffectState::Queued,
                intent_key: "queued-checkpoint".into(),
                attempt: 0,
                reconciliation_attempt: 0,
                fence: 3,
                provider_request_id: None,
                external_resource_ids: BTreeMap::new(),
                next_attempt_at: Some(LATER.into()),
                last_failure: None,
                updated_at: NOW.into(),
            },
            EffectRecord {
                kind: LaunchEffectKind::Diff,
                state: EffectState::FailedRetryable,
                intent_key: "retryable-diff".into(),
                attempt: 1,
                reconciliation_attempt: 0,
                fence: 3,
                provider_request_id: None,
                external_resource_ids: BTreeMap::new(),
                next_attempt_at: Some(LATER.into()),
                last_failure: Some(retryable_failure(NOW)),
                updated_at: NOW.into(),
            },
        ];
        launch.steps[0].status = LaunchStepStatus::Completed;
        launch.steps[1].status = LaunchStepStatus::Failed;
        let retained = (
            launch.fork.clone(),
            launch.branch.clone(),
            launch.workspace.clone(),
            launch.session.clone(),
            launch.diff.clone(),
            launch.contribution.clone(),
            launch.pull_request.clone(),
        );
        let completed_effect = launch.effects[0].clone();
        let owner = launch.owner.clone();
        let digest = launch.effective.digest.clone();
        launch
            .cancel(
                confirmation(ConfirmationAction::Cancel, &owner, &digest),
                LATER,
                format!("cancel-{state:?}"),
            )
            .unwrap();

        assert_eq!(launch.state, State::Cancelled, "state {state:?}");
        assert_eq!(launch.effects[0], completed_effect, "state {state:?}");
        assert_eq!(launch.effects[1].state, EffectState::Cancelled);
        assert_eq!(launch.effects[2].state, EffectState::Cancelled);
        assert_eq!(launch.effects[1].next_attempt_at, None);
        assert_eq!(launch.effects[2].next_attempt_at, None);
        assert_eq!(launch.steps[0].status, LaunchStepStatus::Completed);
        assert_eq!(launch.steps[1].status, LaunchStepStatus::Failed);
        assert!(launch.steps[2..]
            .iter()
            .all(|step| step.status == LaunchStepStatus::Cancelled));
        assert_eq!(
            (
                launch.fork.clone(),
                launch.branch.clone(),
                launch.workspace.clone(),
                launch.session.clone(),
                launch.diff.clone(),
                launch.contribution.clone(),
                launch.pull_request.clone(),
            ),
            retained,
            "state {state:?} must retain every external record"
        );
        let cancelled = launch.clone();
        launch
            .cancel(
                confirmation(ConfirmationAction::Cancel, &owner, &digest),
                LATER,
                "cancel-again",
            )
            .unwrap();
        assert_eq!(launch, cancelled, "cancellation is idempotent in {state:?}");
    }
}

#[test]
fn cancellation_refuses_every_unsettled_external_outcome_atomically() {
    for state in [
        EffectState::Claimed,
        EffectState::Reconciling,
        EffectState::RecoveryRequired,
    ] {
        let mut launch = reviewed_launch();
        launch.effects.push(EffectRecord {
            kind: LaunchEffectKind::PullRequest,
            state,
            intent_key: "unsettled-write".into(),
            attempt: 1,
            reconciliation_attempt: 1,
            fence: 7,
            provider_request_id: None,
            external_resource_ids: BTreeMap::new(),
            next_attempt_at: None,
            last_failure: None,
            updated_at: NOW.into(),
        });
        let before = launch.clone();
        let owner = launch.owner.clone();
        let digest = launch.effective.digest.clone();
        assert!(launch
            .cancel(
                confirmation(ConfirmationAction::Cancel, &owner, &digest),
                LATER,
                "unsafe-cancel",
            )
            .is_err());
        assert_eq!(
            launch, before,
            "unsettled {state:?} must remain recoverable"
        );
    }
}

#[test]
fn cancellation_records_pending_capability_revocation_without_stopping_the_session() {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner.clone()), NOW, "create").unwrap();
    launch.state = LaunchState::Ready;
    launch.session = Some(SessionLink {
        session_id: SessionId("sesn_trigger_cancelled".into()),
        environment_profile_id: "trigger-profile".into(),
        runtime_generation: 1,
        created_at: NOW.into(),
    });
    launch.reach = Some(ReachStatus {
        service_id: "service-1".into(),
        state: CapabilityRevocationState::Active,
        policy_generation: 1,
        policy_fence: 1,
        runtime_audience: "runtime-1".into(),
        expires_at_epoch_seconds: seconds_from_rfc3339(LATER).unwrap(),
        numeric_binding_observed_at: NOW.into(),
        refreshed_at: NOW.into(),
        revocation_requested_at: None,
        revocation_effective_at: None,
        last_refusal: None,
        tombstone_acknowledged: false,
        agent_key_revocation_confirmed: false,
    });

    launch
        .cancel(
            confirmation(ConfirmationAction::Cancel, &owner, &launch.effective.digest),
            LATER,
            "cancel-trigger",
        )
        .unwrap();

    assert_eq!(launch.state, LaunchState::Cancelled);
    assert_eq!(
        launch.reach.as_ref().unwrap().state,
        CapabilityRevocationState::RevocationPending
    );
    assert_eq!(
        launch
            .reach
            .as_ref()
            .unwrap()
            .revocation_requested_at
            .as_deref(),
        Some(LATER)
    );
    assert!(launch.session.is_some());
}

#[test]
fn revoked_status_requires_positive_enforcement_evidence() {
    let reach = ReachStatus {
        service_id: "service-1".into(),
        state: CapabilityRevocationState::Revoked,
        policy_generation: 2,
        policy_fence: 2,
        runtime_audience: "runtime-1".into(),
        expires_at_epoch_seconds: seconds_from_rfc3339(LATER).unwrap(),
        numeric_binding_observed_at: NOW.into(),
        refreshed_at: LATER.into(),
        revocation_requested_at: Some(NOW.into()),
        revocation_effective_at: Some(LATER.into()),
        last_refusal: None,
        tombstone_acknowledged: false,
        agent_key_revocation_confirmed: false,
    };

    assert!(reach.validate().is_err());
}

#[test]
fn workspace_paths_and_symlinks_cannot_escape_the_launch_root() {
    for path in ["../secret", "/absolute", "a//b", "a\\b", "a/./b"] {
        assert!(validate_workspace_path(path).is_err(), "{path} must fail");
    }
    let escaping = WorkspaceEntry {
        path: "link".into(),
        kind: WorkspaceEntryKind::Symlink,
        mode: 0o120000,
        size: 9,
        content_sha256: "a".repeat(64),
        provider_object_id: None,
        symlink_target: Some("../secret".into()),
    };
    assert!(escaping.validate().is_err());
}

#[tokio::test]
async fn publication_is_atomic_and_old_revisions_remain_immutable() {
    let owner = SubjectId("publisher".into());
    let (service, _) = service();
    let draft = service
        .create_draft(
            owner.clone(),
            "service-1".into(),
            content(owner.clone()),
            NOW,
        )
        .await
        .unwrap();
    let first = service
        .publish(&owner, &draft.id, draft.version, "service-1", LATER)
        .await
        .unwrap();

    let mut changed = first.draft.content.clone();
    changed.task.public_summary = "A later revision".into();
    let changed = service
        .update_draft(
            &owner,
            &draft.id,
            first.draft.version,
            "service-1".into(),
            changed,
            LATER,
        )
        .await
        .unwrap();
    let second = service
        .publish(&owner, &draft.id, changed.version, "service-1", LATER)
        .await
        .unwrap();

    assert_ne!(first.revision.id, second.revision.id);
    assert_ne!(first.revision.public_id, second.revision.public_id);
    assert_ne!(first.revision.digest, second.revision.digest);
    assert_eq!(
        first.revision.content.task.public_summary,
        "Fix one bounded issue"
    );
    assert_eq!(first.revision.content.revision_number, 1);
    assert_eq!(second.revision.content.revision_number, 2);
}

#[tokio::test]
async fn publisher_connection_binding_survives_draft_reads_and_replacements() {
    let owner = SubjectId("publisher".into());
    let (service, _) = service();
    let draft = service
        .create_draft(
            owner.clone(),
            "service-original".into(),
            content(owner.clone()),
            NOW,
        )
        .await
        .unwrap();

    assert_eq!(draft.selected_service_id, "service-original");
    assert_eq!(
        service
            .get_draft(&owner, &draft.id)
            .await
            .unwrap()
            .selected_service_id,
        "service-original"
    );

    let replacement = service
        .update_draft(
            &owner,
            &draft.id,
            draft.version,
            "service-reverified".into(),
            draft.content,
            LATER,
        )
        .await
        .unwrap();
    assert_eq!(replacement.selected_service_id, "service-reverified");
    assert_eq!(
        service
            .get_draft(&owner, &draft.id)
            .await
            .unwrap()
            .selected_service_id,
        "service-reverified"
    );
}

#[tokio::test]
async fn publication_refuses_a_connection_other_than_the_draft_binding() {
    let owner = SubjectId("publisher".into());
    let (service, _) = service();
    let draft = service
        .create_draft(
            owner.clone(),
            "service-bound".into(),
            content(owner.clone()),
            NOW,
        )
        .await
        .unwrap();

    let failure = service
        .publish(
            &owner,
            &draft.id,
            draft.version,
            "service-substituted",
            LATER,
        )
        .await
        .unwrap_err();
    assert!(matches!(failure, TriggerServiceError::Validation(_)));
    assert!(failure.to_string().contains("selected_service_id"));

    let unchanged = service.get_draft(&owner, &draft.id).await.unwrap();
    assert_eq!(unchanged.version, draft.version);
    assert_eq!(unchanged.selected_service_id, "service-bound");
}

#[tokio::test]
async fn a_legacy_draft_binds_its_first_authorized_publication_connection() {
    let owner = SubjectId("publisher".into());
    let (service, store) = service();
    let draft = service
        .create_draft(
            owner.clone(),
            "service-pre-migration".into(),
            content(owner.clone()),
            NOW,
        )
        .await
        .unwrap();
    let mut legacy_wire = serde_json::to_value(&draft).unwrap();
    legacy_wire
        .as_object_mut()
        .unwrap()
        .remove("selected_service_id");
    let legacy = serde_json::from_value::<TriggerDraft>(legacy_wire).unwrap();
    assert!(legacy.selected_service_id.is_empty());
    store
        .inner
        .lock()
        .unwrap()
        .drafts
        .insert(legacy.id.clone(), legacy.clone());

    let published = service
        .publish(
            &owner,
            &legacy.id,
            legacy.version,
            "service-authorized",
            LATER,
        )
        .await
        .unwrap();

    assert_eq!(published.draft.selected_service_id, "service-authorized");
    assert_eq!(
        service
            .get_draft(&owner, &legacy.id)
            .await
            .unwrap()
            .selected_service_id,
        "service-authorized"
    );
    assert_eq!(
        published.revision.publisher_authority.connection.service_id,
        "service-authorized"
    );
}

#[tokio::test]
async fn a_readme_reclick_resumes_current_and_new_launch_explicitly_supersedes_it() {
    let publisher = SubjectId("publisher".into());
    let contributor = SubjectId("contributor".into());
    let (service, store) = service();
    let draft = service
        .create_draft(
            publisher.clone(),
            "service-1".into(),
            content(publisher.clone()),
            NOW,
        )
        .await
        .unwrap();
    let published = service
        .publish(&publisher, &draft.id, draft.version, "service-1", NOW)
        .await
        .unwrap();

    let first = service
        .create_or_resume_launch(
            contributor.clone(),
            &published.revision.public_id,
            NOW,
            "first-click",
        )
        .await
        .unwrap();
    let reclick = service
        .create_or_resume_launch(
            contributor.clone(),
            &published.revision.public_id,
            LATER,
            "second-click",
        )
        .await
        .unwrap();
    assert_eq!(first.id, reclick.id);

    let next = service
        .create_new_launch(
            contributor.clone(),
            &published.revision.public_id,
            "new-launch-key-hash",
            LATER,
            "new-launch",
        )
        .await
        .unwrap();
    assert_ne!(first.id, next.id);
    assert!(next.confirmations.iter().any(|confirmation| {
        confirmation.action == ConfirmationAction::NewLaunch
            && confirmation.snapshot_digest == next.revision_digest
    }));
    assert_eq!(
        service
            .create_or_resume_launch(
                contributor,
                &published.revision.public_id,
                LATER,
                "third-click",
            )
            .await
            .unwrap()
            .id,
        next.id
    );
    assert_eq!(
        store
            .replace_launch(first.revision_number, None, &first)
            .await
            .unwrap_err(),
        TriggerStoreError::Superseded
    );
}

#[tokio::test]
async fn a_readme_reclick_after_completion_returns_the_exact_summary_without_new_resources() {
    let publisher = SubjectId("publisher".into());
    let contributor = SubjectId("contributor".into());
    let (service, store) = service();
    let draft = service
        .create_draft(
            publisher.clone(),
            "service-1".into(),
            content(publisher.clone()),
            NOW,
        )
        .await
        .unwrap();
    let published = service
        .publish(&publisher, &draft.id, draft.version, "service-1", NOW)
        .await
        .unwrap();
    let mut completed = service
        .create_or_resume_launch(
            contributor.clone(),
            &published.revision.public_id,
            NOW,
            "first-click",
        )
        .await
        .unwrap();
    completed.state = LaunchState::Completed;
    completed.terminal_at = Some(LATER.into());
    store
        .replace_launch(completed.revision_number, None, &completed)
        .await
        .unwrap();

    let reclick = service
        .create_or_resume_launch(
            contributor,
            &published.revision.public_id,
            LATER,
            "completed-reclick",
        )
        .await
        .unwrap();

    assert_eq!(reclick, completed);
    assert_eq!(store.inner.lock().unwrap().launches.len(), 1);
    assert!(reclick.fork.is_none());
    assert!(reclick.branch.is_none());
    assert!(reclick.workspace.is_none());
    assert!(reclick.session.is_none());
    assert!(reclick.delivery.is_none());
    assert!(reclick.contribution.is_none());
    assert!(reclick.pull_request.is_none());
}

#[tokio::test]
async fn repeated_terminal_cancellation_is_an_exact_no_op_with_a_fresh_command() {
    let publisher = SubjectId("publisher".into());
    let contributor = SubjectId("contributor".into());
    let (service, store) = service();
    let draft = service
        .create_draft(
            publisher.clone(),
            "service-1".into(),
            content(publisher.clone()),
            NOW,
        )
        .await
        .unwrap();
    let published = service
        .publish(&publisher, &draft.id, draft.version, "service-1", NOW)
        .await
        .unwrap();
    let launch = service
        .create_or_resume_launch(
            contributor.clone(),
            &published.revision.public_id,
            NOW,
            "first-click",
        )
        .await
        .unwrap();
    let digest = launch.effective.digest.clone();
    let cancelled = service
        .cancel(
            &contributor,
            &launch.id,
            launch.revision_number,
            confirmation(ConfirmationAction::Cancel, &contributor, &digest),
            LATER,
            "first-cancel",
        )
        .await
        .unwrap();
    let mut repeated_confirmation = confirmation(ConfirmationAction::Cancel, &contributor, &digest);
    repeated_confirmation.idempotency_key_hash = "fresh-cancel-command".into();
    let repeated = service
        .cancel(
            &contributor,
            &launch.id,
            cancelled.revision_number,
            repeated_confirmation,
            LATER,
            "second-cancel",
        )
        .await
        .unwrap();

    assert_eq!(repeated, cancelled);
    assert_eq!(store.inner.lock().unwrap().launches.len(), 1);
    assert_eq!(
        repeated
            .confirmations
            .iter()
            .filter(|confirmation| confirmation.action == ConfirmationAction::Cancel)
            .count(),
        1
    );
}

#[tokio::test]
async fn a_readme_reclick_can_open_the_current_launch_while_rollout_is_paused() {
    let publisher = SubjectId("publisher".into());
    let contributor = SubjectId("contributor".into());
    let (service, store) = service();
    let draft = service
        .create_draft(
            publisher.clone(),
            "service-1".into(),
            content(publisher.clone()),
            NOW,
        )
        .await
        .unwrap();
    let published = service
        .publish(&publisher, &draft.id, draft.version, "service-1", NOW)
        .await
        .unwrap();
    let current = service
        .create_or_resume_launch(
            contributor.clone(),
            &published.revision.public_id,
            NOW,
            "first-click",
        )
        .await
        .unwrap();
    let repository_id = published.revision.content.upstream.repository.repository_id;
    let paused =
        TriggerService::new(store.clone(), store, Arc::new(TestAuthority)).with_rollout(Arc::new(
            TriggerRolloutPolicy::new(
                true,
                true,
                false,
                [publisher.0.clone()],
                [repository_id],
                100,
            )
            .unwrap(),
        ));

    let reclick = paused
        .create_or_resume_launch(
            contributor,
            &published.revision.public_id,
            LATER,
            "paused-reclick",
        )
        .await
        .unwrap();
    assert_eq!(reclick.id, current.id);
    assert_eq!(reclick.revision_number, current.revision_number);

    let denied = paused
        .create_or_resume_launch(
            SubjectId("new-contributor".into()),
            &published.revision.public_id,
            LATER,
            "paused-first-click",
        )
        .await
        .unwrap_err();
    assert!(matches!(
        denied,
        TriggerServiceError::Rollout(TriggerRolloutDenial::OrchestrationPaused)
    ));
}

#[tokio::test]
async fn repeated_paused_start_preserves_the_first_durable_pause_revision() {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner.clone()), NOW, "create").unwrap();
    launch
        .select_connection(connection(), NOW, "select-connection")
        .unwrap();
    let repository_id = launch.revision.content.upstream.repository.repository_id;
    let publisher = launch.revision.content.publisher_subject_id.clone();
    let id = launch.id.clone();
    let digest = launch.effective.digest.clone();
    let (base, store) = service();
    store.insert_new(&launch).await.unwrap();
    let paused = TriggerService::new(store.clone(), store.clone(), Arc::new(TestAuthority))
        .with_rollout(Arc::new(
            TriggerRolloutPolicy::new(true, true, false, [publisher.0], [repository_id], 100)
                .unwrap(),
        ));

    let command = |expected_revision, correlation_id: &str| StartLaunchCommand {
        expected_revision,
        confirmation: confirmation(ConfirmationAction::Start, &owner, &digest),
        acknowledge_workflow_risk: false,
        now: LATER.into(),
        correlation_id: correlation_id.into(),
    };
    assert!(matches!(
        paused
            .start(
                &owner,
                &id,
                command(launch.revision_number, "first-paused-start")
            )
            .await,
        Err(TriggerServiceError::Rollout(
            TriggerRolloutDenial::OrchestrationPaused
        ))
    ));
    let first = base.get_launch(&owner, &id).await.unwrap();
    assert_eq!(
        first.blocked_reason,
        Some(LaunchBlockedReason::FeaturePaused)
    );
    let event_count = first.events.len();

    assert!(matches!(
        paused
            .start(
                &owner,
                &id,
                command(first.revision_number, "second-paused-start")
            )
            .await,
        Err(TriggerServiceError::Rollout(
            TriggerRolloutDenial::OrchestrationPaused
        ))
    ));
    let second = base.get_launch(&owner, &id).await.unwrap();
    assert_eq!(second.revision_number, first.revision_number);
    assert_eq!(second.events.len(), event_count);
    assert_eq!(second.blocked_reason, first.blocked_reason);
}

#[tokio::test]
async fn capability_inventory_excludes_absent_and_effectively_revoked_reach() {
    let (service, store) = service();
    let active = reviewed_launch();
    let active_id = active.id.clone();
    store.insert_new(&active).await.unwrap();

    let mut revoked = reviewed_launch();
    let revoked_id = revoked.id.clone();
    let reach = revoked.reach.as_mut().unwrap();
    reach.state = CapabilityRevocationState::Revoked;
    reach.policy_generation = 2;
    reach.revocation_requested_at = Some(NOW.into());
    reach.revocation_effective_at = Some(LATER.into());
    reach.tombstone_acknowledged = true;
    store.insert_new(&revoked).await.unwrap();

    let mut absent = reviewed_launch();
    let absent_id = absent.id.clone();
    absent.reach = None;
    store.insert_new(&absent).await.unwrap();

    let page = service
        .capability_candidates(TriggerPageRequest::default())
        .await
        .unwrap();
    let ids = page
        .items
        .into_iter()
        .map(|launch| launch.id)
        .collect::<Vec<_>>();
    assert!(ids.contains(&active_id));
    assert!(!ids.contains(&revoked_id));
    assert!(!ids.contains(&absent_id));
}

#[tokio::test]
async fn expired_worker_takeover_fences_the_old_worker() {
    let owner = SubjectId("contributor".into());
    let (service, store) = service();
    let launch = Launch::new(owner.clone(), revision(owner), NOW, "create").unwrap();
    store.insert_new(&launch).await.unwrap();

    let first = service
        .acquire_worker(
            &launch.id,
            launch.revision_number,
            "worker-a",
            "2026-08-26T12:00:00Z",
            "2026-08-26T12:00:30Z",
        )
        .await
        .unwrap()
        .unwrap();
    let first_lease = first.lease.clone().unwrap();
    let second = service
        .acquire_worker(
            &launch.id,
            first.revision_number,
            "worker-b",
            "2026-08-26T12:00:31Z",
            "2026-08-26T12:01:00Z",
        )
        .await
        .unwrap()
        .unwrap();
    assert!(second.fencing_counter > first.fencing_counter);

    let error = service
        .commit_worker(first.revision_number, &first_lease, first)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        TriggerServiceError::Store(TriggerStoreError::StaleFence { .. })
    ));
}

#[test]
fn effect_claims_persist_one_intent_and_reconcile_after_a_crash() {
    let (mut launch, lease) = leased_started_launch();
    let first = launch
        .claim_next_effect(&lease, NOW, "claim")
        .unwrap()
        .unwrap();
    assert_eq!(first.kind, LaunchEffectKind::Preflight);
    assert_eq!(first.mode, EffectClaimMode::Execute);
    assert_eq!(first.attempt, 1);
    assert_eq!(first.reconciliation_attempt, 0);
    assert_eq!(launch.effects.len(), 1);
    assert_eq!(launch.effects[0].state, EffectState::Claimed);
    let step = launch
        .steps
        .iter()
        .find(|step| step.key == LaunchStepKey::Preflight)
        .unwrap();
    assert_eq!(step.status, LaunchStepStatus::Running);
    assert_eq!(step.worker_fencing_token, lease.fencing_token);

    let replay = launch
        .claim_next_effect(&lease, LATER, "reconcile")
        .unwrap()
        .unwrap();
    assert_eq!(replay.mode, EffectClaimMode::Reconcile);
    assert_eq!(replay.intent_key, first.intent_key);
    assert_eq!(replay.attempt, first.attempt);
    assert_eq!(replay.reconciliation_attempt, 1);
    assert_eq!(launch.effects.len(), 1);

    launch
        .complete_effect(
            &lease,
            &replay,
            EffectCompletion {
                result: LaunchEffectResult::Preflight {
                    connection: connection(),
                    authority: launch.authority.clone().unwrap(),
                    source_sha: "a".repeat(40),
                    workflow_paths: Vec::new(),
                },
                safe_detail: "Authority is current.".into(),
                external_resource_ids: Default::default(),
                now: LATER.into(),
                correlation_id: "complete".into(),
            },
        )
        .unwrap();
    assert_eq!(launch.effects[0].state, EffectState::Succeeded);
    assert_eq!(launch.state, LaunchState::ForkPending);
    assert_eq!(
        launch
            .steps
            .iter()
            .find(|step| step.key == LaunchStepKey::Preflight)
            .unwrap()
            .status,
        LaunchStepStatus::Completed
    );
}

#[test]
fn replacement_worker_reconciles_the_same_intent_and_fences_the_old_claim() {
    let (mut launch, first_lease) = leased_started_launch();
    let first = launch
        .claim_next_effect(&first_lease, NOW, "first")
        .unwrap()
        .unwrap();
    let second_lease = WorkerLease {
        holder: "worker-b".into(),
        fencing_token: 2,
        acquired_at: LATER.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = 2;
    launch.lease = Some(second_lease.clone());

    assert!(launch
        .complete_effect(
            &first_lease,
            &first,
            EffectCompletion {
                result: LaunchEffectResult::Preflight {
                    connection: connection(),
                    authority: launch.authority.clone().unwrap(),
                    source_sha: "a".repeat(40),
                    workflow_paths: Vec::new(),
                },
                safe_detail: "must not commit".into(),
                external_resource_ids: Default::default(),
                now: LATER.into(),
                correlation_id: "stale".into(),
            },
        )
        .is_err());
    let takeover = launch
        .claim_next_effect(&second_lease, LATER, "takeover")
        .unwrap()
        .unwrap();
    assert_eq!(takeover.mode, EffectClaimMode::Reconcile);
    assert_eq!(takeover.intent_key, first.intent_key);
    assert_eq!(takeover.fence, 2);
    assert_eq!(takeover.attempt, 1);
    assert_eq!(takeover.reconciliation_attempt, 1);
}

#[test]
fn retry_deadlines_and_budgets_are_durable_and_do_not_spend_early() {
    let (mut launch, lease) = leased_started_launch();
    let claim = launch
        .claim_next_effect(&lease, NOW, "claim")
        .unwrap()
        .unwrap();
    launch
        .fail_effect(
            &lease,
            &claim,
            retryable_failure(NOW),
            Some("2026-08-26T12:02:00Z".into()),
            NOW,
        )
        .unwrap();
    assert_eq!(launch.state, LaunchState::FailedRetryable);
    assert_eq!(launch.effects[0].attempt, 1);

    assert_eq!(
        launch
            .claim_next_effect(&lease, LATER, "too-early")
            .unwrap(),
        None
    );
    assert_eq!(launch.state, LaunchState::FailedRetryable);
    assert_eq!(launch.effects[0].attempt, 1);

    let retry = launch
        .claim_next_effect(&lease, "2026-08-26T12:02:00Z", "retry")
        .unwrap()
        .unwrap();
    assert_eq!(retry.mode, EffectClaimMode::Execute);
    assert_eq!(retry.attempt, 2);
    assert_eq!(launch.state, LaunchState::Preflight);

    launch.effects[0].state = EffectState::FailedRetryable;
    launch.effects[0].attempt = MAX_LAUNCH_EFFECT_ATTEMPTS;
    launch.effects[0].next_attempt_at = Some("2026-08-26T12:03:00Z".into());
    launch.state = LaunchState::FailedRetryable;
    launch.resume_state = Some(LaunchState::Preflight);
    launch.failure = Some(retryable_failure("2026-08-26T12:02:01Z"));
    assert_eq!(
        launch
            .claim_next_effect(&lease, "2026-08-26T12:03:00Z", "exhausted")
            .unwrap(),
        None
    );
    assert_eq!(launch.state, LaunchState::FailedRetryable);
    assert_eq!(launch.effects[0].attempt, MAX_LAUNCH_EFFECT_ATTEMPTS);
}

fn leased_started_launch() -> (Launch, WorkerLease) {
    let owner = SubjectId("contributor".into());
    let mut launch = Launch::new(owner.clone(), revision(owner.clone()), NOW, "create").unwrap();
    launch
        .select_connection(connection(), NOW, "connection")
        .unwrap();
    launch.authority = Some(AuthorityProvenance {
        subject: owner.clone(),
        subject_token_generation: 1,
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "key-1".into(),
        session_credential_generation: None,
        state: DelegationState::Available,
        observed_at: NOW.into(),
    });
    launch
        .start(
            confirmation(ConfirmationAction::Start, &owner, &launch.effective.digest),
            NOW,
            "start",
        )
        .unwrap();
    let lease = WorkerLease {
        holder: "worker-a".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    launch.fencing_counter = 1;
    launch.lease = Some(lease.clone());
    (launch, lease)
}

fn retryable_failure(observed_at: &str) -> LaunchFailure {
    LaunchFailure {
        class: FailureClass::RetryableTransient,
        code: "provider_temporarily_unavailable".into(),
        detail: "The provider is temporarily unavailable.".into(),
        remediation: "CMA will retry this step after the shown deadline.".into(),
        correlation_id: "retry-test".into(),
        retry_at: None,
        observed_at: observed_at.into(),
    }
}

fn content(owner: SubjectId) -> TriggerRevisionContent {
    TriggerRevisionContent {
        schema_version: TRIGGER_SCHEMA_VERSION,
        trigger_id: TriggerId::new(),
        revision_number: 1,
        publisher_subject_id: owner,
        publisher_display_identity: "Chrono Maintainer".into(),
        upstream: UpstreamRevision {
            repository: upstream(),
            source_mode: SourceMode::PinnedCommit,
            observed_source_sha: Some("a".repeat(40)),
            contribution_base_ref: "main".into(),
        },
        task: TriggerTask {
            public_summary: "Fix one bounded issue".into(),
            prompt_template: "Implement {{topic}} and run its tests.".into(),
            renderer_version: PROMPT_RENDERER_VERSION,
            input_fields: vec![InputFieldDefinition {
                key: "topic".into(),
                kind: InputFieldKind::Text {
                    min_bytes: 1,
                    max_bytes: 100,
                },
                required: true,
                default: Some(json!("the selected issue")),
                editable: true,
            }],
        },
        session: TriggerSessionPolicy {
            required_capabilities: vec![
                RequiredCapability::Conversation,
                RequiredCapability::DurableConversation,
                RequiredCapability::WorkspaceCheckpoint,
                RequiredCapability::RestrictedServiceGateway,
                RequiredCapability::CredentialRefresh,
                RequiredCapability::CredentialRevocation,
            ],
            permission_ceiling: PermissionMode::Autonomous,
            sandbox_profile: "cma-trigger-v1".into(),
            resources: ResourcePolicy {
                default: SessionSize::new(500, 1_024).unwrap(),
                ceiling: SessionSize::new(1_000, 2_048).unwrap(),
            },
            timeout_default_seconds: 3_600,
            timeout_ceiling_seconds: 7_200,
        },
        repository_access: RepositoryAccessPolicy {
            credential_route: CredentialRoute::ContributorConnection,
            required_scope_classes: vec![ScopeClass::PublicRepository],
            allowed_operation_classes: vec![
                RepositoryOperationClass::RepositoryRead,
                RepositoryOperationClass::TreeRead,
                RepositoryOperationClass::ObjectCreate,
                RepositoryOperationClass::LaunchRefCreate,
                RepositoryOperationClass::LaunchRefUpdate,
                RepositoryOperationClass::StatusRead,
            ],
            workflow_change_policy: WorkflowChangePolicy::Refuse,
        },
        setup: SetupPolicy {
            executor_policy: ExecutorPolicy::Disabled,
            steps: Vec::new(),
        },
        contribution: ContributionPolicy {
            branch_prefix: "cma/".into(),
            commit_attribution: CommitAttributionPolicy::ContributorNoReply,
            pull_request: PullRequestDefaults {
                title_template: "CMA contribution".into(),
                body_template: "Generated from an explicitly reviewed CMA Launch.".into(),
            },
        },
        commercial_policy: CommercialPolicy {
            billing_mode: BillingMode::ZeroCost,
            policy_reference: "public-zero-cost-v1".into(),
        },
        override_policy: OverridePolicy {
            editable_fields: vec![EditableField::TaskInput],
            optional_checks: Vec::new(),
        },
        public_projection: PublicProjectionPolicy { version: 1 },
    }
}

fn revision(owner: SubjectId) -> TriggerRevision {
    let content = content(owner);
    TriggerRevision {
        id: TriggerRevisionId::new(),
        public_id: PublicTriggerId::new(),
        digest: content.digest().unwrap(),
        publisher_authority: publisher_authority(&content.publisher_subject_id),
        content,
        availability: TriggerAvailability::Published,
        published_at: NOW.into(),
        retired_at: None,
    }
}

pub(crate) fn revision_fixture(owner: SubjectId) -> TriggerRevision {
    revision(owner)
}

fn publisher_authority(subject: &SubjectId) -> PublisherAuthority {
    PublisherAuthority {
        subject: subject.clone(),
        subject_token_generation: 1,
        connection: connection(),
        repository: upstream(),
        role: PublisherRepositoryRole::Owner,
        verified_at: NOW.into(),
    }
}

fn upstream() -> RepositoryIdentity {
    RepositoryIdentity {
        repository_id: 10,
        repository_node_id: "R_upstream".into(),
        owner_id: 20,
        owner_node_id: "U_upstream_owner".into(),
        canonical_owner: "upstream-owner".into(),
        canonical_name: "project".into(),
        visibility: RepositoryVisibility::Public,
        allow_forking: true,
        default_branch: "main".into(),
        selected_ref: "main".into(),
        fork_parent_id: None,
        fork_source_id: None,
    }
}

fn fork() -> RepositoryIdentity {
    RepositoryIdentity {
        repository_id: 11,
        repository_node_id: "R_fork".into(),
        owner_id: 21,
        owner_node_id: "U_contributor".into(),
        canonical_owner: "contributor".into(),
        canonical_name: "project".into(),
        visibility: RepositoryVisibility::Public,
        allow_forking: true,
        default_branch: "main".into(),
        selected_ref: "main".into(),
        fork_parent_id: Some(10),
        fork_source_id: Some(10),
    }
}

fn connection() -> ConnectionSnapshot {
    ConnectionSnapshot {
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "key-1".into(),
        account: ConnectedAccount {
            account_id: 21,
            account_node_id: "U_contributor".into(),
            login: "contributor".into(),
            avatar_url: None,
        },
        state: ConnectionState::Active,
        granted_scopes: vec!["public_repo".into()],
        satisfied_scope_classes: vec![ScopeClass::PublicRepository],
        consent_observed_at: NOW.into(),
        preflight_observed_at: NOW.into(),
    }
}

fn confirmation(action: ConfirmationAction, owner: &SubjectId, snapshot: &str) -> Confirmation {
    Confirmation {
        action,
        subject: owner.clone(),
        snapshot_digest: snapshot.into(),
        confirmed_at: NOW.into(),
        idempotency_key_hash: "idempotency-digest".into(),
    }
}

fn capability() -> LaunchCapability {
    LaunchCapability {
        version: CAPABILITY_POLICY_VERSION,
        launch_id: LaunchId::new(),
        session_id: crate::SessionId::generate(),
        runtime_audience: "runtime-generation-1".into(),
        service_id: "service-1".into(),
        upstream: upstream(),
        fork: fork(),
        allowed_ref: "refs/heads/cma/launch".into(),
        operations: vec![CapabilityOperation::Repository(
            RepositoryOperationClass::RepositoryRead,
        )],
        generation: 1,
        fence: 1,
        not_before_epoch_seconds: 100,
        expires_at_epoch_seconds: 160,
        issuer: "cma".into(),
        audience: "layer4".into(),
        signature_key_id: "trigger-policy-v1".into(),
        numeric_binding_observed_at: NOW.into(),
        revocation: CapabilityRevocationState::Active,
    }
}

fn service() -> (TriggerService, Arc<MemoryTriggerStore>) {
    let store = Arc::new(MemoryTriggerStore::default());
    let service = TriggerService::new(store.clone(), store.clone(), Arc::new(TestAuthority));
    (service, store)
}

struct TestAuthority;

#[async_trait]
impl TriggerAuthorityVerifier for TestAuthority {
    async fn verify_publisher(
        &self,
        _subject: &SubjectId,
        content: &TriggerRevisionContent,
        selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        let mut authority = publisher_authority(&content.publisher_subject_id);
        authority.connection.service_id = selected_service_id.to_string();
        Ok(authority)
    }

    async fn verify_launch(
        &self,
        _subject: &SubjectId,
        _revision: &TriggerRevision,
        selected_service_id: &str,
        previous: Option<&ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError> {
        let mut connection = connection();
        connection.service_id = selected_service_id.to_string();
        connection.validate_for(&[ScopeClass::PublicRepository])?;
        if previous.is_some_and(|old| old.account.account_id != connection.account.account_id) {
            return Err(TriggerAuthorityError::AuthorityDrift.into());
        }
        Ok(LaunchAuthority {
            provenance: AuthorityProvenance {
                subject: SubjectId("contributor".into()),
                subject_token_generation: 1,
                service_id: connection.service_id.clone(),
                service_state_version: connection.service_state_version,
                backing_key_id: connection.backing_key_id.clone(),
                session_credential_generation: None,
                state: DelegationState::Available,
                observed_at: NOW.into(),
            },
            connection,
            source_sha: "a".repeat(40),
            workflow_paths: Vec::new(),
        })
    }
}

#[derive(Default)]
struct MemoryTriggerStore {
    inner: Mutex<MemoryData>,
}

#[derive(Default)]
struct MemoryData {
    drafts: HashMap<TriggerId, TriggerDraft>,
    revisions: HashMap<TriggerRevisionId, TriggerRevision>,
    public: HashMap<PublicTriggerId, TriggerRevisionId>,
    launches: HashMap<LaunchId, Launch>,
    current: HashMap<(String, String), LaunchId>,
}

#[async_trait]
impl TriggerStore for MemoryTriggerStore {
    async fn insert_draft(&self, draft: &TriggerDraft) -> Result<(), TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        if data
            .drafts
            .insert(draft.id.clone(), draft.clone())
            .is_some()
        {
            return Err(TriggerStoreError::AlreadyExists);
        }
        Ok(())
    }

    async fn get_draft(&self, id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError> {
        Ok(self.inner.lock().unwrap().drafts.get(id).cloned())
    }

    async fn list_drafts_for(
        &self,
        owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerStoreError> {
        Ok(TriggerPage {
            items: self
                .inner
                .lock()
                .unwrap()
                .drafts
                .values()
                .filter(|draft| &draft.owner == owner)
                .cloned()
                .collect(),
            next: None,
        })
    }

    async fn replace_draft(
        &self,
        expected_version: u64,
        draft: &TriggerDraft,
    ) -> Result<TriggerDraft, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        let actual = data
            .drafts
            .get(&draft.id)
            .ok_or_else(|| TriggerStoreError::Malformed("draft disappeared".into()))?
            .version;
        if actual != expected_version {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_version,
                actual,
            });
        }
        data.drafts.insert(draft.id.clone(), draft.clone());
        Ok(draft.clone())
    }

    async fn publish_revision(
        &self,
        expected_draft_version: u64,
        draft: &TriggerDraft,
        revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        let actual = data
            .drafts
            .get(&draft.id)
            .ok_or_else(|| TriggerStoreError::Malformed("draft disappeared".into()))?
            .version;
        if actual != expected_draft_version {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_draft_version,
                actual,
            });
        }
        if data.revisions.contains_key(&revision.id)
            || data.public.contains_key(&revision.public_id)
        {
            return Err(TriggerStoreError::AlreadyExists);
        }
        data.drafts.insert(draft.id.clone(), draft.clone());
        data.public
            .insert(revision.public_id.clone(), revision.id.clone());
        data.revisions.insert(revision.id.clone(), revision.clone());
        Ok(PublishedRevision {
            draft: draft.clone(),
            revision: revision.clone(),
        })
    }

    async fn get_revision(
        &self,
        id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok(self.inner.lock().unwrap().revisions.get(id).cloned())
    }

    async fn get_public_revision(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        let data = self.inner.lock().unwrap();
        Ok(data
            .public
            .get(id)
            .and_then(|revision_id| data.revisions.get(revision_id))
            .cloned())
    }

    async fn list_revisions_for(
        &self,
        owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerStoreError> {
        Ok(TriggerPage {
            items: self
                .inner
                .lock()
                .unwrap()
                .revisions
                .values()
                .filter(|revision| &revision.content.publisher_subject_id == owner)
                .cloned()
                .collect(),
            next: None,
        })
    }

    async fn retire_revision(
        &self,
        id: &TriggerRevisionId,
        owner: &SubjectId,
        retired_at: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        let Some(revision) = data.revisions.get_mut(id) else {
            return Ok(None);
        };
        if &revision.content.publisher_subject_id != owner {
            return Ok(None);
        }
        revision.availability = TriggerAvailability::Retired;
        revision.retired_at = Some(retired_at.into());
        Ok(Some(revision.clone()))
    }
}

#[async_trait]
impl LaunchStore for MemoryTriggerStore {
    async fn insert_or_get_current(&self, launch: &Launch) -> Result<Launch, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        let key = current_key(launch);
        if let Some(id) = data.current.get(&key) {
            return data
                .launches
                .get(id)
                .cloned()
                .ok_or_else(|| TriggerStoreError::Malformed("current Launch disappeared".into()));
        }
        data.current.insert(key, launch.id.clone());
        data.launches.insert(launch.id.clone(), launch.clone());
        Ok(launch.clone())
    }

    async fn get_current_launch(
        &self,
        owner: &SubjectId,
        revision: &TriggerRevisionId,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let data = self.inner.lock().unwrap();
        Ok(data
            .current
            .get(&(owner.0.clone(), revision.0.clone()))
            .and_then(|id| data.launches.get(id))
            .cloned())
    }

    async fn insert_new(&self, launch: &Launch) -> Result<(), TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        if data.launches.contains_key(&launch.id) {
            return Err(TriggerStoreError::AlreadyExists);
        }
        data.current.insert(current_key(launch), launch.id.clone());
        data.launches.insert(launch.id.clone(), launch.clone());
        Ok(())
    }

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError> {
        Ok(self.inner.lock().unwrap().launches.get(id).cloned())
    }

    async fn list_launches(
        &self,
        query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        let data = self.inner.lock().unwrap();
        Ok(TriggerPage {
            items: data
                .launches
                .values()
                .filter(|launch| {
                    query
                        .owner
                        .as_ref()
                        .is_none_or(|owner| &launch.owner == owner)
                        && (query.states.is_empty() || query.states.contains(&launch.state))
                })
                .cloned()
                .collect(),
            next: None,
        })
    }

    async fn replace_launch(
        &self,
        expected_revision: u64,
        expected_fence: Option<u64>,
        launch: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        if data.current.get(&current_key(launch)) != Some(&launch.id) {
            return Err(TriggerStoreError::Superseded);
        }
        let current = data
            .launches
            .get(&launch.id)
            .ok_or_else(|| TriggerStoreError::Malformed("Launch disappeared".into()))?;
        if let Some(expected) = expected_fence {
            if current.fencing_counter != expected {
                return Err(TriggerStoreError::StaleFence {
                    expected,
                    actual: current.fencing_counter,
                });
            }
        }
        if current.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision_number,
            });
        }
        if expected_fence.is_none() && current.lease.is_some() {
            return Err(TriggerStoreError::LeaseHeld);
        }
        data.launches.insert(launch.id.clone(), launch.clone());
        Ok(launch.clone())
    }

    async fn acquire_lease(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        let Some(current) = data.launches.get(id) else {
            return Ok(None);
        };
        if data.current.get(&current_key(current)) != Some(id)
            || current.revision_number != expected_revision
            || current
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at.as_str() > acquired_at)
        {
            return Ok(None);
        }
        let launch = data.launches.get_mut(id).unwrap();
        launch.revision_number = launch.revision_number.checked_add(1).unwrap();
        launch.fencing_counter = launch.fencing_counter.checked_add(1).unwrap();
        launch.lease = Some(WorkerLease {
            holder: holder.into(),
            fencing_token: launch.fencing_counter,
            acquired_at: acquired_at.into(),
            expires_at: expires_at.into(),
        });
        Ok(Some(launch.clone()))
    }

    async fn release_lease(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        lease: &WorkerLease,
    ) -> Result<Launch, TriggerStoreError> {
        let mut data = self.inner.lock().unwrap();
        let launch = data
            .launches
            .get_mut(id)
            .ok_or_else(|| TriggerStoreError::Malformed("Launch disappeared".into()))?;
        if launch.fencing_counter != lease.fencing_token {
            return Err(TriggerStoreError::StaleFence {
                expected: lease.fencing_token,
                actual: launch.fencing_counter,
            });
        }
        if launch.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: launch.revision_number,
            });
        }
        launch.revision_number += 1;
        launch.lease = None;
        Ok(launch.clone())
    }
}

fn current_key(launch: &Launch) -> (String, String) {
    (launch.owner.0.clone(), launch.revision.id.0.clone())
}
