use crate::sandbox::lifecycle_test_support::{agent, checkpoint, sandbox, NOW};
use crate::sandbox::store::{
    checkpoint_capture::validate_checkpoint_capture_work, AgentStore, AgentWorkerStore,
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard, SandboxStore, SandboxStoreError, SandboxTransaction,
    SandboxTransactionStore, SandboxWorkerStore,
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
fn only_exact_runtime_loss_discards_capture_debt_without_resume() {
    let mut value = active();
    value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
    value.begin_checkpoint_capture(LATER.into()).unwrap();
    value.recover_runtime_loss(&[], LATER.into()).unwrap();
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

fn active() -> Sandbox {
    let mut value = sandbox();
    value.runtime.as_mut().unwrap().provision_generation = Some(1);
    value.lifecycle.runtime_generation_high_water = 1;
    value
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
