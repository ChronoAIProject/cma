use super::*;
use crate::sandbox::store::{
    retired_sandbox_cleanup_is_absent, retiring_sandbox_process_is_absent,
    validate_agent_work_mutation_with_sandbox,
};
use crate::sandbox::{
    Sandbox, SandboxCheckpointConversation, SandboxCheckpointRef, SandboxStatus, SandboxWorkStep,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

#[test]
fn retired_sandbox_delete_proof_is_scoped_to_terminal_owner_and_cleanup_steps() {
    let (mut agent, _, mut sandbox) = fixture();
    agent.status = AgentStatus::Stopped;
    agent.requested_stop_reason = None;
    agent.work.step = AgentWorkStep::RemoveHome;
    sandbox
        .transition(SandboxStatus::Retired, AT.into(), None)
        .unwrap();
    sandbox
        .work
        .schedule(SandboxWorkStep::WaitForPurge, AT.into());
    assert!(retired_sandbox_cleanup_is_absent(&agent, &sandbox));
    agent.conversation.active_turn = Some(9);
    assert!(retired_sandbox_cleanup_is_absent(&agent, &sandbox));
    for variant in 0..8 {
        let mut agent = agent.clone();
        let mut sandbox = sandbox.clone();
        match variant {
            0 => agent.owner = SubjectId::new("other-owner"),
            1 => sandbox.id = SandboxId("other-sandbox".into()),
            2 => agent.status = AgentStatus::Ready,
            3 => agent.work.step = AgentWorkStep::OpenConversation,
            4 => sandbox.status = SandboxStatus::Paused,
            5 => sandbox.retired_at = None,
            6 => sandbox.runtime = crate::sandbox::lifecycle_test_support::sandbox().runtime,
            _ => agent
                .work
                .cleanup_debt
                .push(debt(&agent, AgentWorkStep::MaterializeHome)),
        }
        assert!(
            !retired_sandbox_cleanup_is_absent(&agent, &sandbox),
            "variant {variant}"
        );
    }
}

fn fixture() -> (Agent, Worktree, Sandbox) {
    let (mut agent, worktree) = claimed(
        AgentWorkStep::CloseProcess,
        AgentStatus::Paused,
        WorktreeStatus::Ready,
    );
    add_startup_receipts(&mut agent);
    agent.requested_stop_reason = Some(AgentStopReason::SandboxRetired);
    let mut sandbox = crate::sandbox::lifecycle_test_support::sandbox();
    sandbox.id = agent.sandbox_id.clone();
    sandbox.owner = agent.owner.clone();
    sandbox.runtime = None;
    sandbox.lifecycle.runtime_generation_high_water = 1;
    let archive = b"retained conversation".to_vec();
    let conversation_id = agent.conversation.conversation_id.clone().unwrap();
    let conversation = SandboxCheckpointConversation::new(
        agent.id.clone(),
        conversation_id.clone(),
        ConversationSnapshot {
            conversation_id,
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    sandbox.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoint/retired".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &[conversation],
            NOW,
        )
        .unwrap(),
    );
    sandbox.status = SandboxStatus::Paused;
    sandbox.begin_retirement(AT.into()).unwrap();
    assert!(sandbox.validate().is_ok());
    (agent, worktree, sandbox)
}

#[test]
fn retired_runtime_close_preserves_history_without_fabricating_a_runtime_receipt() {
    let (agent, worktree, sandbox) = fixture();
    let mutation = completed_advance(&agent, AgentWorkStep::PublishStopped);
    assert!(retiring_sandbox_process_is_absent(&agent, &sandbox));
    assert_conflict(&agent, &worktree, &mutation);
    assert_eq!(
        validate_agent_work_mutation_with_sandbox(
            &agent,
            &worktree,
            agent.conversation.last_seq,
            &mutation,
            &sandbox,
        ),
        Ok(())
    );
    assert_eq!(mutation.agent.work.receipts, agent.work.receipts);
    assert_eq!(
        mutation.agent.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(
        mutation.agent.conversation.active_turn,
        agent.conversation.active_turn
    );
    let mut forged = mutation.clone();
    let mut close_receipt = receipt(&agent, AgentWorkStep::CloseProcess);
    close_receipt.completed_at = AT.into();
    forged.agent.work.receipts.push(close_receipt);
    assert_eq!(
        validate_agent_work_mutation_with_sandbox(
            &agent,
            &worktree,
            agent.conversation.last_seq,
            &forged,
            &sandbox,
        ),
        Err(SandboxStoreError::Conflict)
    );
    let mut stale = mutation;
    stale.fence += 1;
    assert_eq!(
        validate_agent_work_mutation_with_sandbox(
            &agent,
            &worktree,
            agent.conversation.last_seq,
            &stale,
            &sandbox,
        ),
        Err(SandboxStoreError::Conflict)
    );
}

#[test]
fn retired_runtime_close_requires_exact_quiet_retirement_and_checkpoint_provenance() {
    for variant in 0..18 {
        let (mut agent, worktree, mut sandbox) = fixture();
        match variant {
            0 => sandbox.status = SandboxStatus::Active,
            1 => sandbox.status = SandboxStatus::Rotating,
            2 => sandbox.status = SandboxStatus::Paused,
            3 => sandbox.runtime = crate::sandbox::lifecycle_test_support::sandbox().runtime,
            4 => sandbox.lifecycle.checkpoints.latest = None,
            5 => sandbox.owner = SubjectId::new("another-owner"),
            6 => agent.status = AgentStatus::Ready,
            7 => agent.requested_stop_reason = None,
            8 => agent.conversation.runtime_generation = Some(3),
            9 => agent.conversation.conversation_id = Some("another-conversation".into()),
            10 => sandbox.work.step = SandboxWorkStep::TeardownRuntime,
            11 => agent
                .work
                .cleanup_debt
                .push(debt(&agent, AgentWorkStep::OpenConversation)),
            12 => {
                sandbox
                    .lifecycle
                    .checkpoints
                    .latest
                    .as_mut()
                    .unwrap()
                    .manifest_sha256 = "b".repeat(64)
            }
            13 => agent.conversation.active_turn = Some(1),
            14 => agent.conversation.queued_messages.push(
                crate::sandbox::QueuedAgentMessage::new(1, "pending message".into(), NOW.into())
                    .unwrap(),
            ),
            15 => {
                sandbox.lifecycle.runtime_provision =
                    Some(crate::sandbox::SandboxRuntimeProvisionReservation {
                        operation_id: "provision-pending".into(),
                        generation: 2,
                    })
            }
            16 => {
                let mut pausing = crate::sandbox::lifecycle_test_support::sandbox();
                pausing.begin_pause(AT.into(), None).unwrap();
                sandbox.lifecycle.operation = pausing.lifecycle.operation;
            }
            _ => {
                let mut capturing = crate::sandbox::lifecycle_test_support::sandbox();
                capturing.runtime.as_mut().unwrap().provision_generation = Some(1);
                capturing.lifecycle.runtime_generation_high_water = 1;
                capturing.begin_checkpoint_capture(AT.into()).unwrap();
                sandbox.lifecycle.checkpoint_capture = capturing.lifecycle.checkpoint_capture;
            }
        }
        assert!(
            !retiring_sandbox_process_is_absent(&agent, &sandbox),
            "variant {variant}"
        );
        let mutation = completed_advance(&agent, AgentWorkStep::PublishStopped);
        assert_eq!(
            validate_agent_work_mutation_with_sandbox(
                &agent,
                &worktree,
                agent.conversation.last_seq,
                &mutation,
                &sandbox,
            ),
            Err(SandboxStoreError::Conflict),
            "variant {variant}"
        );
    }
}

#[test]
fn pending_owner_stop_uses_parent_retirement_proof_without_changing_attribution() {
    let (mut agent, worktree, sandbox) = fixture();
    for reason in [AgentStopReason::Owner, AgentStopReason::BindingRetired] {
        agent.requested_stop_reason = Some(reason.clone());
        let mutation = completed_advance(&agent, AgentWorkStep::PublishStopped);
        assert_eq!(
            validate_agent_work_mutation_with_sandbox(
                &agent,
                &worktree,
                agent.conversation.last_seq,
                &mutation,
                &sandbox,
            ),
            Ok(())
        );
        assert_eq!(mutation.agent.requested_stop_reason, Some(reason));
        assert_eq!(mutation.agent.work.receipts, agent.work.receipts);
        let mut unretired = sandbox.clone();
        unretired.status = SandboxStatus::Paused;
        assert!(!retiring_sandbox_process_is_absent(&agent, &unretired));
    }
}

#[test]
fn retired_runtime_close_accepts_the_parked_fence_after_generation_three() {
    let (mut agent, worktree, mut sandbox) = fixture();
    sandbox.lifecycle.runtime_generation_high_water = 3;
    agent.conversation.runtime_generation = Some(4);
    let mutation = completed_advance(&agent, AgentWorkStep::PublishStopped);
    assert_eq!(
        validate_agent_work_mutation_with_sandbox(
            &agent,
            &worktree,
            agent.conversation.last_seq,
            &mutation,
            &sandbox,
        ),
        Ok(())
    );
    agent.conversation.runtime_generation = Some(5);
    assert!(!retiring_sandbox_process_is_absent(&agent, &sandbox));
    agent.conversation.runtime_generation = Some(1);
    sandbox.lifecycle.runtime_generation_high_water = 0;
    assert!(!retiring_sandbox_process_is_absent(&agent, &sandbox));
    sandbox.lifecycle.runtime_generation_high_water = u64::MAX;
    assert!(!retiring_sandbox_process_is_absent(&agent, &sandbox));
}
