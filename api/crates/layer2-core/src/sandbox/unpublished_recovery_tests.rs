use super::*;
use crate::sandbox::lifecycle_test_support::{agent, NOW};
use crate::sandbox::runtime_update_test_support::native;
use crate::sandbox::{
    AgentOperationIntent, AgentOperationRecord, AgentWorkReceipt, AgentWorkStep,
    SandboxRotationIdentity, SandboxUnpublishedRecoveryQuiescence, TypedFailure,
};

#[path = "unpublished_recovery_lifecycle_tests.rs"]
mod lifecycle;

#[path = "unpublished_recovery_history_tests.rs"]
mod history;

#[path = "unpublished_recovery_stalled_tests.rs"]
mod stalled;

#[path = "unpublished_recovery_reservation_tests.rs"]
mod reservation;

#[path = "unpublished_recovery_failed_tests.rs"]
mod failed;
#[path = "unpublished_recovery_legacy_tests.rs"]
mod legacy;
#[path = "runtime_update_scoped_tests.rs"]
mod scoped_restart;

fn snapshot(parked_update: bool) -> SandboxUnpublishedRecoverySnapshot {
    let mut sandbox = native();
    let mut held = agent("agt_recovery", "native-original", 0);
    held.status = AgentStatus::Paused;
    held.conversation.runtime_generation = Some(1);
    held.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", held.id),
        request_sha256: "d".repeat(64),
        completed_at: NOW.into(),
    });
    if parked_update {
        sandbox
            .request_runtime_update("image-v2".into(), false, NOW.into())
            .unwrap();
        let identity = SandboxRotationIdentity::capture(&sandbox, &[held.clone()]).unwrap();
        sandbox
            .begin_image_rotation("image-v2".into(), identity, NOW.into())
            .unwrap();
        let Some(SandboxLifecycleOperation::ImageRotation { phase, .. }) =
            &mut sandbox.lifecycle.operation
        else {
            panic!()
        };
        *phase = SandboxImageRotationPhase::ConversationReopen;
        sandbox
            .lifecycle
            .runtime_update
            .as_mut()
            .unwrap()
            .request
            .as_mut()
            .unwrap()
            .failure = Some(failure("terminal_validation"));
    } else {
        sandbox.status = SandboxStatus::Failed;
        sandbox.failure = Some(failure("restore_failed"));
    }
    sandbox.lifecycle.runtime_generation_high_water = 2;
    sandbox.runtime.as_mut().unwrap().provision_generation = Some(2);
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    let snapshot = SandboxUnpublishedRecoverySnapshot {
        sandbox,
        agents: vec![held],
        workspaces: vec![],
    };
    snapshot.validate().unwrap();
    snapshot
}

fn failure(class: &str) -> TypedFailure {
    TypedFailure {
        class: class.into(),
        detail: "native reopen did not complete".into(),
    }
}

fn request(snapshot: SandboxUnpublishedRecoverySnapshot) -> SandboxUnpublishedRecoveryRequest {
    SandboxUnpublishedRecoveryRequest {
        proof: SandboxUnpublishedRecoveryProof {
            sandbox_id: snapshot.sandbox.id.clone(),
            runtime_id: snapshot
                .sandbox
                .unpublished_recovery_binding()
                .unwrap()
                .runtime_id
                .clone(),
            runtime_generation: snapshot.runtime_generation().unwrap(),
            reserved_provision: None,
            observed_provision_generation: None,
            home_identity: snapshot.home_identity().unwrap(),
            boot_id: snapshot
                .sandbox
                .unpublished_recovery_binding()
                .unwrap()
                .boot_id
                .clone(),
            runtime_process_absent: false,
            requires_native_quiescence: true,
            provider_plan: serde_json::json!({"inspection": "test-process-snapshot"}),
            provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(
                &serde_json::json!({"inspection": "test-process-snapshot"}),
            )
            .unwrap(),
            conversations: snapshot
                .agents
                .iter()
                .filter(|agent| agent.status.is_live())
                .map(|agent| SandboxRecoveryConversationProof {
                    agent_id: agent.id.clone(),
                    conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                    history_sha256: "e".repeat(64),
                })
                .collect(),
        },
        snapshot,
        target_runtime_version: "image-v3".into(),
        released_runtime: None,
        automatic: false,
        requested_at: NOW.into(),
    }
}

#[test]
fn failed_reopen_recovery_keeps_original_home_and_every_admitted_child_record() {
    for parked_update in [false, true] {
        let mut before = snapshot(parked_update);
        before.agents[0].conversation.queued_messages.push(
            crate::sandbox::QueuedAgentMessage::new(1, "retained owner input".into(), NOW.into())
                .unwrap(),
        );
        let request = request(before.clone());
        let mutation = apply_unpublished_runtime_recovery(&before, &request).unwrap();
        let after = &mutation.sandbox;
        assert_eq!(after.status, SandboxStatus::Rotating);
        assert_eq!(after.lifecycle.home, before.sandbox.lifecycle.home);
        assert_eq!(
            after.lifecycle.suspended_runtime.as_deref(),
            before.sandbox.runtime.as_ref()
        );
        assert!(after.runtime.is_none());
        assert_eq!(after.retained_home_recovery().unwrap().source_generation, 2);
        assert_eq!(after.admitted_runtime_update_target(), Some("image-v3"));
        assert_eq!(after.lifecycle.runtime_generation_high_water, 2);
        assert_eq!(request.snapshot.agents, before.agents);
        assert_eq!(
            after.lifecycle.checkpoints,
            before.sandbox.lifecycle.checkpoints
        );
        assert_eq!(after.credential, before.sandbox.credential);
        assert!(matches!(
            after.lifecycle.operation,
            Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::CheckpointVerification,
                ..
            })
        ));
        assert!(matches!(
            mutation.event,
            SandboxEventKind::RuntimeRecoveryAdmitted { .. }
        ));
        after.validate().unwrap();
    }
}

#[test]
fn no_current_generation_turn_or_publication_can_enter_recovery() {
    for case in 0..13 {
        let mut held = snapshot(true);
        let agent = &mut held.agents[0];
        match case {
            0 => agent.status = AgentStatus::Ready,
            1 => agent.status = AgentStatus::Working,
            2 => agent.conversation.active_turn = Some(8),
            3 => agent.conversation.runtime_generation = Some(2),
            4 => agent
                .work
                .schedule(AgentWorkStep::DispatchFirstMessage, NOW.into()),
            5 => {
                agent.work.lease = Some(crate::sandbox::AgentWorkLease {
                    holder: HOLDER.into(),
                    fence: 6,
                    expires_at: END.into(),
                });
            }
            6 => agent.conversation.conversation_id = None,
            7 => agent.requested_stop_reason = Some(crate::sandbox::AgentStopReason::Owner),
            8 => {
                agent.conversation.operation = Some(
                    AgentOperationRecord::new(
                        1,
                        2,
                        2,
                        AgentOperationIntent::send_message("already admitted"),
                        NOW.into(),
                    )
                    .unwrap(),
                )
            }
            9 => {
                let mut first =
                    crate::sandbox::FirstMessageRecord::new("recovery-test", "pending", NOW)
                        .unwrap();
                first.fence_dispatch_for(1, 1, 6, NOW.into()).unwrap();
                agent.conversation.first_message = Some(first);
            }
            10 => held
                .sandbox
                .work
                .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into()),
            11 => held.sandbox.status = SandboxStatus::Active,
            12 => {
                let mut stopped = agent.clone();
                stopped.id = AgentId("agt_stopped".into());
                stopped.status = AgentStatus::Stopped;
                stopped.conversation.runtime_generation = Some(2);
                held.agents.push(stopped);
            }
            _ => unreachable!(),
        }
        assert!(held.validate().is_err(), "case {case}");
    }
}

#[test]
fn native_proof_is_exact_to_boot_home_and_complete_original_conversation_set() {
    for case in 0..13 {
        let held = snapshot(false);
        let mut request = request(held.clone());
        match case {
            0 => request.proof.boot_id = None,
            1 => request.proof.boot_id = Some("f".repeat(32)),
            2 => request.proof.home_identity = "foreign-home".into(),
            3 => request.proof.runtime_generation += 1,
            4 => request.proof.runtime_id = "another-runtime".into(),
            5 => request.proof.conversations.clear(),
            6 => request.proof.conversations[0].conversation_id = "another-conversation".into(),
            7 => request
                .proof
                .conversations
                .push(request.proof.conversations[0].clone()),
            8 => request.proof.conversations[0].history_sha256 = "not-a-digest".into(),
            9 => request.proof.requires_native_quiescence = false,
            10 => request.proof.provider_plan = serde_json::Value::Null,
            11 => request.proof.provider_plan_sha256 = "a".repeat(64),
            12 => request.proof.provider_plan = serde_json::json!({"too_large": "a".repeat(65536)}),
            _ => unreachable!(),
        }
        assert!(
            apply_unpublished_runtime_recovery(&held, &request).is_err(),
            "case {case}"
        );
    }
}

const HOLDER: &str = "recovery-worker";
const END: &str = "2026-09-05T01:00:00Z";

fn admitted() -> Sandbox {
    let held = snapshot(true);
    let request = request(held.clone());
    apply_unpublished_runtime_recovery(&held, &request)
        .unwrap()
        .sandbox
}

fn quiescence(sandbox: &Sandbox) -> SandboxUnpublishedRecoveryQuiescence {
    let recovery = sandbox.lifecycle.unpublished_recovery.as_ref().unwrap();
    let proof = &recovery.proof;
    let mut conversations = proof.conversations.clone();
    // Graceful completion may append a final output/turn-aborted record.
    if let Some(conversation) = conversations.first_mut() {
        conversation.history_sha256 = "a".repeat(64);
    }
    SandboxUnpublishedRecoveryQuiescence {
        operation_id: recovery.operation_id.clone(),
        runtime_id: proof.runtime_id.clone(),
        runtime_generation: proof.runtime_generation,
        home_identity: proof.home_identity.clone(),
        boot_id: proof.boot_id.clone(),
        provider_plan_sha256: proof.provider_plan_sha256.clone(),
        runtime_process_absent: true,
        conversations,
        completed_at: NOW.into(),
    }
}

#[test]
fn admission_cannot_suspend_or_advance_until_quiescence_is_durable() {
    let mut sandbox = admitted();
    let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let before = sandbox.clone();
    let source = sandbox.retained_home_recovery().unwrap().clone();
    assert!(sandbox.require_unpublished_runtime_quiescence().is_err());
    assert!(sandbox
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .is_err());
    assert!(sandbox
        .reserve_lifecycle_runtime_generation(3, HOLDER, fence, NOW.into())
        .is_err());
    assert!(sandbox
        .fail_lifecycle("unproven completion".into(), HOLDER, fence, NOW.into())
        .is_err());
    assert_eq!(sandbox, before);
    let mut bypass = sandbox.clone();
    bypass.lifecycle.unpublished_recovery = None;
    assert!(sandbox
        .validate_runtime_generation_successor(&bypass)
        .is_err());
    let receipt = quiescence(&sandbox);
    sandbox
        .complete_unpublished_runtime_quiescence(receipt, HOLDER, fence, NOW.into())
        .unwrap();
    let mutation = crate::sandbox::store::SandboxLifecycleMutation {
        guard: crate::sandbox::store::SandboxMutationGuard::capture(&before),
        sandbox: sandbox.clone(),
        agents: vec![],
        agent_scope: crate::sandbox::store::SandboxLifecycleAgentScope::Touched,
        appended_at: NOW.into(),
        events: vec![
            SandboxEventKind::RuntimeRecoveryQuiesced {
                receipt: Box::new(
                    sandbox
                        .lifecycle
                        .unpublished_recovery
                        .as_ref()
                        .unwrap()
                        .quiescence
                        .clone()
                        .unwrap(),
                ),
            },
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            },
        ],
    };
    crate::sandbox::store::validate_lifecycle_mutation(&before, &[], &mutation).unwrap();
    assert!(sandbox.work.lease.is_none());
    let encoded = serde_json::to_vec(&sandbox).unwrap();
    let mut resumed: Sandbox = serde_json::from_slice(&encoded).unwrap();
    resumed.require_unpublished_runtime_quiescence().unwrap();
    let fence = resumed.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    resumed
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = resumed.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    assert_eq!(
        resumed
            .reserve_lifecycle_runtime_generation(3, HOLDER, fence, NOW.into())
            .unwrap(),
        3
    );
}

#[test]
fn graceful_receipt_must_match_original_operation_boot_plan_and_conversations() {
    for case in 0..11 {
        let mut sandbox = admitted();
        let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
        let original = sandbox.clone();
        let mut receipt = quiescence(&sandbox);
        match case {
            0 => receipt.operation_id = "foreign-operation".into(),
            1 => receipt.runtime_id = "foreign-runtime".into(),
            2 => receipt.runtime_generation += 1,
            3 => receipt.home_identity = "foreign-home".into(),
            4 => receipt.boot_id = Some("f".repeat(32)),
            5 => receipt.provider_plan_sha256 = "f".repeat(64),
            6 => receipt.runtime_process_absent = false,
            7 => receipt.conversations.clear(),
            8 => receipt.conversations[0].history_sha256 = "invalid".into(),
            9 => receipt.completed_at = END.into(),
            10 => receipt.completed_at = "2026-09-05T00:00:00Z".into(),
            _ => unreachable!(),
        }
        assert!(
            sandbox
                .complete_unpublished_runtime_quiescence(receipt, HOLDER, fence, NOW.into())
                .is_err(),
            "case {case}"
        );
        assert_eq!(sandbox, original);
    }
    let mut sandbox = admitted();
    let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let receipt = quiescence(&sandbox);
    let original = sandbox.clone();
    assert!(sandbox
        .complete_unpublished_runtime_quiescence(receipt, HOLDER, fence + 1, NOW.into())
        .is_err());
    assert_eq!(sandbox, original);
}

#[test]
fn evidence_cannot_survive_concurrent_queue_or_child_changes() {
    let held = snapshot(true);
    let request = request(held.clone());
    for case in 0..3 {
        let mut current = held.clone();
        match case {
            0 => current.agents[0].revision += 1,
            1 => current.agents[0].conversation.queued_messages.push(
                crate::sandbox::QueuedAgentMessage::new(1, "newly admitted".into(), NOW.into())
                    .unwrap(),
            ),
            2 => current.agents.clear(),
            _ => unreachable!(),
        }
        assert!(matches!(
            apply_unpublished_runtime_recovery(&current, &request),
            Err(SandboxStoreError::Conflict)
        ));
    }
}

#[test]
fn no_agent_recovery_accepts_verified_home_without_a_live_supervisor() {
    let mut held = snapshot(false);
    held.agents.clear();
    held.sandbox.failure = Some(failure("provisioning_failed"));
    let mut request = request(held.clone());
    request.proof.boot_id = None;
    request.proof.runtime_process_absent = true;
    assert!(apply_unpublished_runtime_recovery(&held, &request).is_ok());
    held.sandbox.runtime = None;
    assert!(
        held.validate().is_err(),
        "no original runtime authority is never fabricated"
    );
}

#[test]
fn unfinished_empty_workspace_requires_process_absence_and_keeps_prepare_pending() {
    use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
    use crate::sandbox::{WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState};
    let mut held = snapshot(false);
    held.agents.clear();
    held.sandbox.failure = Some(failure("provisioning_failed"));
    let workspace = Workspace {
        id: crate::WorkspaceId("wks_recovery".into()),
        sandbox_id: held.sandbox.id.clone(),
        owner: held.sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: crate::WorkspaceProfileId("wkp_recovery".into()),
            revision: 1,
            digest: "a".repeat(64),
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
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    };
    held.workspaces.push(workspace.clone());
    let mut request = request(held.clone());
    assert!(apply_unpublished_runtime_recovery(&held, &request).is_err());
    request.proof.boot_id = None;
    request.proof.runtime_process_absent = true;
    let after = apply_unpublished_runtime_recovery(&held, &request).unwrap();
    assert_eq!(request.snapshot.workspaces, vec![workspace]);
    assert!(after
        .sandbox
        .require_unpublished_runtime_quiescence()
        .is_err());
    for step in [WorkspaceWorkStep::Setup, WorkspaceWorkStep::Remove] {
        let mut changed = held.clone();
        changed.workspaces[0].work.step = step;
        assert!(changed.validate().is_err());
    }
    held.workspaces[0]
        .work
        .claim(HOLDER.into(), NOW, END.into())
        .unwrap();
    assert!(held.validate().is_err());
}

#[test]
fn existing_owner_revive_and_active_runtime_update_eligibility_are_unchanged() {
    let held = snapshot(false);
    assert!(held.sandbox.revive_eligibility().is_err());
    let mut active = native();
    assert!(!active.unpublished_recovery_candidate());
    assert!(active
        .request_runtime_update("image-v3".into(), false, NOW.into())
        .unwrap());
    assert_eq!(active.status, SandboxStatus::Active);
}

fn stalled_revival() -> SandboxUnpublishedRecoverySnapshot {
    let mut sandbox = native();
    sandbox.recover_runtime_loss(&[], true, NOW.into()).unwrap();
    let source = sandbox.retained_home_recovery().unwrap().clone();
    let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    sandbox
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    sandbox
        .reserve_lifecycle_runtime_generation(2, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = sandbox.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    sandbox
        .retry_lifecycle(NOW.into(), HOLDER, fence, NOW.into())
        .unwrap();
    SandboxUnpublishedRecoverySnapshot {
        sandbox,
        agents: vec![],
        workspaces: vec![],
    }
}

#[test]
fn no_agent_stalled_revival_preserves_source_and_reservation_high_water() {
    let held = stalled_revival();
    held.validate().unwrap();
    assert_eq!(held.runtime_generation().unwrap(), 1);
    assert_eq!(held.sandbox.lifecycle.runtime_generation_high_water, 2);
    let mut request = request(held.clone());
    assert!(apply_unpublished_runtime_recovery(&held, &request).is_err());
    request.proof.boot_id = None;
    request.proof.runtime_process_absent = true;
    let mut after = apply_unpublished_runtime_recovery(&held, &request)
        .unwrap()
        .sandbox;
    assert_eq!(
        after.lifecycle.suspended_runtime,
        held.sandbox.lifecycle.suspended_runtime
    );
    assert_eq!(after.lifecycle.home, held.sandbox.lifecycle.home);
    assert_eq!(after.retained_home_recovery().unwrap().source_generation, 1);
    assert_eq!(after.lifecycle.runtime_generation_high_water, 2);
    assert!(after.lifecycle.runtime_provision.is_none());
    let receipt = quiescence(&after);
    let fence = after.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    after
        .complete_unpublished_runtime_quiescence(receipt, HOLDER, fence, NOW.into())
        .unwrap();
    let source = after.retained_home_recovery().unwrap().clone();
    let fence = after.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    after
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = after.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    assert_eq!(
        after
            .reserve_lifecycle_runtime_generation(2, HOLDER, fence, NOW.into())
            .unwrap(),
        3
    );
}

#[test]
fn stalled_revival_refuses_any_agent_lease_missing_source_or_unproved_absence() {
    let held = stalled_revival();
    for deleted in [false, true] {
        let mut changed = held.clone();
        let mut child = agent("agt_never_ignore", "original", 0);
        if deleted {
            child.status = AgentStatus::Deleted;
        }
        changed.agents.push(child);
        assert!(changed.validate().is_err());
    }
    let mut leased = held.clone();
    leased
        .sandbox
        .work
        .claim(HOLDER.into(), NOW, END.into())
        .unwrap();
    assert!(leased.validate().is_err());
    let mut missing = held.clone();
    missing.sandbox.lifecycle.suspended_runtime = None;
    assert!(missing.validate().is_err());
    let mut request = request(held.clone());
    request.proof.boot_id = None;
    assert!(apply_unpublished_runtime_recovery(&held, &request).is_err());
    request.proof.runtime_process_absent = true;
    request.proof.runtime_generation = 2;
    assert!(apply_unpublished_runtime_recovery(&held, &request).is_err());
}

#[test]
fn no_agent_receipt_transaction_rechecks_complete_agent_set() {
    use crate::sandbox::store::{
        validate_lifecycle_mutation, SandboxLifecycleAgentScope, SandboxLifecycleMutation,
        SandboxMutationGuard,
    };
    let held = stalled_revival();
    let mut request = request(held.clone());
    request.proof.boot_id = None;
    request.proof.runtime_process_absent = true;
    let mut current = apply_unpublished_runtime_recovery(&held, &request)
        .unwrap()
        .sandbox;
    let fence = current.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    let receipt = quiescence(&current);
    let mut next = current.clone();
    next.complete_unpublished_runtime_quiescence(receipt.clone(), HOLDER, fence, NOW.into())
        .unwrap();
    let mutation = SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(&current),
        sandbox: next,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: NOW.into(),
        events: vec![
            SandboxEventKind::RuntimeRecoveryQuiesced {
                receipt: Box::new(receipt),
            },
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            },
        ],
    };
    validate_lifecycle_mutation(&current, &[], &mutation).unwrap();
    for deleted in [false, true] {
        let mut added = agent("agt_arrived_after_admission", "old-conversation", 0);
        if deleted {
            added.status = AgentStatus::Deleted;
        }
        assert!(validate_lifecycle_mutation(&current, &[(added, 0)], &mutation).is_err());
    }
}
