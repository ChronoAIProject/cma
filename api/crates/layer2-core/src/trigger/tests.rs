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
                LaunchEffectResult::Checkpoint {
                    checkpoint: malformed,
                },
                "must not attach",
                Default::default(),
                LATER,
                "reject-checkpoint",
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
            LaunchEffectResult::Checkpoint {
                checkpoint: valid.clone(),
            },
            "checkpoint complete",
            Default::default(),
            LATER,
            "complete-checkpoint",
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
            LaunchEffectResult::BranchPublication {
                contribution: replaced_intent,
            },
            "branch published",
            BTreeMap::from([
                ("base_tree_id".into(), "b".repeat(40)),
                ("tree_id".into(), "c".repeat(40)),
                ("commit_id".into(), "d".repeat(40)),
                ("resulting_head".into(), "d".repeat(40)),
            ]),
            LATER,
            "replaced-publication-intent",
        )
        .is_err());
    assert_eq!(launch, before, "publication completion is intent-bound");
    launch
        .complete_effect(
            &lease,
            &claim,
            LaunchEffectResult::BranchPublication {
                contribution: published.clone(),
            },
            "branch published",
            BTreeMap::from([
                ("base_tree_id".into(), "b".repeat(40)),
                ("tree_id".into(), "c".repeat(40)),
                ("commit_id".into(), "d".repeat(40)),
                ("resulting_head".into(), "d".repeat(40)),
            ]),
            LATER,
            "publication-complete",
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
        .create_draft(owner.clone(), content(owner.clone()), NOW)
        .await
        .unwrap();
    let first = service
        .publish(&owner, &draft.id, draft.version, "service-1", LATER)
        .await
        .unwrap();

    let mut changed = first.draft.content.clone();
    changed.task.public_summary = "A later revision".into();
    let changed = service
        .update_draft(&owner, &draft.id, first.draft.version, changed, LATER)
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
async fn a_readme_reclick_resumes_current_and_new_launch_explicitly_supersedes_it() {
    let publisher = SubjectId("publisher".into());
    let contributor = SubjectId("contributor".into());
    let (service, store) = service();
    let draft = service
        .create_draft(publisher.clone(), content(publisher.clone()), NOW)
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
            LaunchEffectResult::Preflight {
                connection: connection(),
                authority: launch.authority.clone().unwrap(),
                source_sha: "a".repeat(40),
                workflow_paths: Vec::new(),
            },
            "Authority is current.",
            Default::default(),
            LATER,
            "complete",
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
            LaunchEffectResult::Preflight {
                connection: connection(),
                authority: launch.authority.clone().unwrap(),
                source_sha: "a".repeat(40),
                workflow_paths: Vec::new(),
            },
            "must not commit",
            Default::default(),
            LATER,
            "stale",
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
