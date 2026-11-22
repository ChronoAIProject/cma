use crate::sandbox::lifecycle_test_support::{agent, checkpoint, sandbox, IDLE_AT, NOW};
use crate::sandbox::store::{
    validate_lifecycle_mutation, AgentConversationMutation, AgentConversationStore, AgentStore,
    InMemorySandboxStore, SandboxLifecycleAgentScope, SandboxLifecycleCommand,
    SandboxLifecycleCommandStore, SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
};
use crate::sandbox::*;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

const HOLDER: &str = "home-recovery-worker";
const END: &str = "2026-09-05T01:00:00Z";

fn native() -> Sandbox {
    let mut value = sandbox();
    let mut home = SandboxHome::reserve(
        &value.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "empty".into(),
            template_generation: "original-seed".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "original-disk".into(),
        storage_generation: "disk-generation".into(),
    })
    .unwrap();
    value.lifecycle.home = Some(Box::new(home));
    value.lifecycle.runtime_generation_high_water = 10;
    value.runtime.as_mut().unwrap().provision_generation = Some(10);
    value.validate().unwrap();
    value
}

fn lost_pause() -> Sandbox {
    let mut value = native();
    value
        .begin_pause(IDLE_AT.into(), Some("Paused by you.".into()))
        .unwrap();
    value.recover_runtime_loss(&[], false, NOW.into()).unwrap();
    value
}

fn claim(value: &mut Sandbox) -> u64 {
    value.work.claim(HOLDER.into(), NOW, END.into()).unwrap()
}

fn mutation(current: &Sandbox, next: Sandbox) -> SandboxLifecycleMutation {
    let events = if current.status != next.status {
        vec![SandboxEventKind::StatusChanged {
            from: current.status,
            to: next.status,
        }]
    } else {
        vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }]
    };
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox: next,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: NOW.into(),
        events,
    }
}

#[test]
fn loss_before_first_snapshot_pins_original_disk_without_inventing_an_archive() {
    let mut value = native();
    let original = value.clone();
    value.recover_runtime_loss(&[], false, NOW.into()).unwrap();
    assert_eq!(value.status, SandboxStatus::Reviving);
    assert!(value.runtime.is_none());
    assert!(value.lifecycle.checkpoints.latest.is_none());
    assert_eq!(value.credential, original.credential);
    assert_eq!(value.lifecycle.home, original.lifecycle.home);
    assert_eq!(
        value.retained_home_recovery().unwrap().source_generation,
        10
    );
    assert!(matches!(
        value.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::RuntimeLost,
            checkpoint_epoch: None,
            ..
        })
    ));
    let saved = serde_json::to_vec(&value).unwrap();
    assert_eq!(serde_json::from_slice::<Sandbox>(&saved).unwrap(), value);
}

#[test]
fn recovery_uses_disk_even_when_a_completed_older_archive_exists() {
    let mut value = native();
    value.lifecycle.checkpoints.latest = Some(checkpoint(1, IDLE_AT));
    value.recover_runtime_loss(&[], false, NOW.into()).unwrap();
    let fence = claim(&mut value);
    let original = value.clone();
    let archive = value.lifecycle.checkpoints.latest.clone().unwrap();
    assert!(value
        .complete_lifecycle_checkpoint_verification(&archive, HOLDER, fence, NOW.into())
        .is_err());
    assert_eq!(value, original);
    let source = value.retained_home_recovery().unwrap().clone();
    let mut forged = source.clone();
    forged.home_identity = "f".repeat(64);
    assert!(value
        .complete_lifecycle_home_verification(&forged, HOLDER, fence, NOW.into())
        .is_err());
    assert_eq!(value, original);
    value
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let valid = mutation(&original, value.clone());
    validate_lifecycle_mutation(&original, &[], &valid).unwrap();
    let mut forged = valid.clone();
    forged
        .sandbox
        .lifecycle
        .home_recovery
        .as_mut()
        .unwrap()
        .source_generation -= 1;
    assert!(validate_lifecycle_mutation(&original, &[], &forged).is_err());
    let mut forged = valid;
    forged.sandbox.lifecycle.next_authority += 1;
    assert!(validate_lifecycle_mutation(&original, &[], &forged).is_err());
}

#[test]
fn interrupted_pause_returns_to_pausing_with_new_authority_and_no_live_publication() {
    let mut value = lost_pause();
    let source = value.retained_home_recovery().unwrap();
    assert_eq!(
        source.resume_pause.as_ref().unwrap().notice.as_deref(),
        Some("Paused by you.")
    );
    if let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
        value.lifecycle.operation.as_mut()
    {
        *phase = SandboxRevivePhase::PublishActive;
    }
    let mut binding = native().runtime.unwrap();
    binding.provision_generation = Some(11);
    value.runtime = Some(binding);
    value.lifecycle.runtime_generation_high_water = 11;
    let fence = claim(&mut value);
    let original = value.clone();
    value
        .publish_lifecycle_active(END.into(), HOLDER, fence, NOW.into())
        .unwrap();
    assert_eq!(value.status, SandboxStatus::Pausing);
    assert_eq!(
        value.lifecycle.next_authority,
        original.lifecycle.next_authority + 1
    );
    let retained = value
        .retained_home_recovery()
        .expect("the resumed pause keeps the retained home authority");
    assert_eq!(retained.source_generation, 11);
    assert!(retained.resume_pause.is_none());
    assert_eq!(value.work.ready_at, NOW);
    assert_eq!(
        value.lifecycle.paused_notice.as_deref(),
        Some("Paused by you.")
    );
    validate_lifecycle_mutation(&original, &[], &mutation(&original, value)).unwrap();
}

#[test]
fn repeated_runtime_loss_preserves_pause_but_new_creation_cancels_it() {
    let mut value = lost_pause();
    value.runtime = native().runtime;
    value.recover_runtime_loss(&[], false, NOW.into()).unwrap();
    assert!(value
        .retained_home_recovery()
        .unwrap()
        .resume_pause
        .is_some());
    let original = value.clone();
    value
        .record_creation_admission(SandboxReviveCause::AgentCreation, NOW.into())
        .unwrap();
    assert!(value
        .retained_home_recovery()
        .unwrap()
        .resume_pause
        .is_none());
    assert_eq!(value.lifecycle.operation, original.lifecycle.operation);
    assert_eq!(value.revision, original.revision + 1);
    let replay = value.clone();
    assert!(!value.cancel_recovery_pause(NOW).unwrap());
    assert_eq!(value, replay);
}

#[test]
fn ready_publication_keeps_the_original_conversation_and_startup_receipts() {
    let value = native();
    let mut starting = agent("agt_starting", "original-conversation", 0);
    starting.status = AgentStatus::Starting;
    starting.ready_at = None;
    starting.work.step = AgentWorkStep::PublishReady;
    starting.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: "agent:agt_starting:open:v1".into(),
        request_sha256: "a".repeat(64),
        completed_at: IDLE_AT.into(),
    });
    starting.conversation.first_message =
        Some(FirstMessageRecord::new("launch-original", "First prompt", IDLE_AT).unwrap());
    let (next, logs) =
        crate::sandbox::runtime_recovery_agents::recover_agent(&value, &starting, NOW, false)
            .unwrap();
    assert_eq!(next.status, AgentStatus::Starting);
    assert_eq!(next.work.step, AgentWorkStep::PublishReady);
    assert_eq!(next.work.receipts, starting.work.receipts);
    assert_eq!(
        next.conversation.conversation_id,
        starting.conversation.conversation_id
    );
    assert_eq!(
        next.conversation.first_message,
        starting.conversation.first_message
    );
    assert_eq!(next.conversation.runtime_generation, Some(5));
    assert_eq!(
        logs,
        vec![AgentLogKind::RuntimeGenerationRetired {
            previous_generation: 4,
            runtime_generation: 5
        }]
    );
    let mut recovering = value;
    recovering
        .recover_runtime_loss(std::slice::from_ref(&next), false, NOW.into())
        .unwrap();
    assert!(!recovering.restores_agent(&next));
}

#[tokio::test]
async fn owner_revive_command_durably_cancels_the_remembered_pause() {
    let store = InMemorySandboxStore::default();
    let mut value = lost_pause();
    value.revision = 1;
    store.create_sandbox(value.clone()).await.unwrap();
    store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            value.id.clone(),
            value.owner,
            NOW.into(),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap();
    let next = store.get_sandbox(&value.id).await.unwrap().unwrap();
    assert_eq!(next.status, SandboxStatus::Reviving);
    assert!(next
        .retained_home_recovery()
        .unwrap()
        .resume_pause
        .is_none());
}

#[tokio::test]
async fn queued_message_and_pause_cancellation_commit_together() {
    let store = InMemorySandboxStore::default();
    let mut original = lost_pause();
    original.revision = 1;
    store.create_sandbox(original.clone()).await.unwrap();
    let parent = original.clone();
    let mut current = agent("agt_a", "conversation-a", 0);
    current.status = AgentStatus::Paused;
    current.revision = 1;
    store.create_agent(current.clone()).await.unwrap();
    let mut next = current.clone();
    next.conversation
        .queued_messages
        .push(QueuedAgentMessage::new(1, "New owner work".into(), NOW.into()).unwrap());
    next.conversation.last_seq = 1;
    next.revision += 1;
    next.updated_at = NOW.into();
    let request = AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: Some(SandboxMutationGuard::capture(&parent)),
        expected_agent_revision: current.revision,
        expected_last_seq: 0,
        runtime_generation: 4,
        agent: next.clone(),
        appended_at: NOW.into(),
        agent_logs: vec![AgentLogKind::OwnerMessage {
            content: "New owner work".into(),
        }],
    };
    let mut rejected = request.clone();
    rejected.expected_agent_revision += 1;
    assert!(store.commit_agent_conversation(rejected).await.is_err());
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    store.commit_agent_conversation(request).await.unwrap();
    let recovered = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert!(recovered
        .retained_home_recovery()
        .unwrap()
        .resume_pause
        .is_none());
    assert_eq!(recovered.revision, parent.revision + 1);
    assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(next));
}
