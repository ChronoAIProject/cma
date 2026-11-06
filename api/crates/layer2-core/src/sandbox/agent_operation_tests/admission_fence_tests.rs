use std::sync::atomic::AtomicUsize;

use async_trait::async_trait;
use layer3_adapter::{ConversationInputSender, ConversationUpdate, PromptResponse, TurnStopReason};

use crate::sandbox::store::{
    AgentConversationMutation, SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore,
};
use crate::SandboxId;

use super::*;

#[derive(Clone, Copy)]
enum ParentChange {
    Pause,
    Retire,
    Revision,
    Checkpoint,
}

struct ChangingParentRegistry {
    store: Arc<InMemorySandboxStore>,
    sender: ConversationInputSender,
    resolution: AtomicUsize,
    change_at: usize,
    change: ParentChange,
}

#[async_trait]
impl AgentRuntimeRegistry for ChangingParentRegistry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let resolution = self.resolution.fetch_add(1, Ordering::SeqCst) + 1;
        if resolution == self.change_at {
            let id = SandboxId("sbx_operations".into());
            let mut sandbox = self.store.get_sandbox(&id).await.unwrap().unwrap();
            let expected = sandbox.revision;
            match self.change {
                ParentChange::Checkpoint => {
                    begin_capture(&self.store, sandbox).await;
                    return Ok(AgentRuntimeConnection {
                        capabilities: full_capabilities(),
                        input: self.sender.clone(),
                    });
                }
                ParentChange::Pause => sandbox.begin_pause(LATER.into(), None).unwrap(),
                ParentChange::Retire => sandbox.begin_retirement(LATER.into()).unwrap(),
                ParentChange::Revision => sandbox.revision += 1,
            }
            self.store.save_sandbox(expected, sandbox).await.unwrap();
        }
        Ok(AgentRuntimeConnection {
            capabilities: full_capabilities(),
            input: self.sender.clone(),
        })
    }

    async fn fence(&self, _: &crate::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

#[tokio::test]
async fn checkpoint_between_policy_read_and_commit_refuses_without_retry_or_admission() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut parent = crate::sandbox::lifecycle_test_support::sandbox();
    parent.id = SandboxId("sbx_operations".into());
    parent.revision = 1;
    parent.runtime.as_mut().unwrap().provision_generation = Some(1);
    parent.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(parent).await.unwrap();
    let mut ready = test_agent(AgentStatus::Ready, None);
    ready.conversation.last_seq = 1;
    store.create_agent(ready.clone()).await.unwrap();
    let prior = store
        .append_agent_log(
            &ready.id,
            "2026-09-04T23:58:00Z".into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None,
            },
        )
        .await
        .unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = changing_registry(store.clone(), sender, ParentChange::Checkpoint, 1);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    assert_eq!(
        service
            .send_message(&owner(), &ready.id, "Not accepted".into())
            .await,
        Err(AgentOperationError::CheckpointInProgress)
    );
    assert_eq!(registry.resolution.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    assert_eq!(
        store.agent_log(&ready.id, 0, 100).await.unwrap(),
        vec![prior]
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(
        service
            .send_message(&owner(), &ready.id, "Still not accepted".into())
            .await,
        Err(AgentOperationError::CheckpointInProgress)
    );
    assert_eq!(registry.resolution.load(Ordering::SeqCst), 1);
}

async fn begin_capture(store: &InMemorySandboxStore, parent: Sandbox) {
    let mut next = parent.clone();
    next.begin_checkpoint_capture(LATER.into()).unwrap();
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&parent),
            sandbox: next,
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: LATER.into(),
            events: vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn parent_pause_between_policy_read_and_commit_queues_without_runtime_dispatch() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = changing_registry(store.clone(), sender, ParentChange::Pause, 1);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    let result = service
        .send_message(&owner(), &ready.id, "Preserve this request".into())
        .await
        .unwrap();
    let AgentMessageOutcome::Queued { agent, sequence } = result else {
        panic!("stale parent admitted runtime input");
    };
    assert_eq!(sequence, 1);
    assert_eq!(agent.revision, ready.revision + 1);
    assert_eq!(agent.conversation.operation, None);
    assert_eq!(agent.conversation.active_turn, None);
    assert_eq!(
        agent.conversation.queued_messages[0].content,
        "Preserve this request"
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(registry.resolution.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .get_sandbox(&ready.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
    let logs = store.agent_log(&ready.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    assert!(
        matches!(&logs[0].kind, AgentLogKind::OwnerMessage { content } if content == "Preserve this request")
    );
}

#[tokio::test]
async fn parent_revision_change_retries_admission_without_duplicating_input_or_logs() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = changing_registry(store.clone(), sender, ParentChange::Revision, 1);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "Exactly once".into())
            .await
            .unwrap(),
        AgentMessageOutcome::Dispatched(_)
    ));
    assert_eq!(registry.resolution.load(Ordering::SeqCst), 3);
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let logs = store.agent_log(&ready.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(
                entry.kind,
                AgentLogKind::OperationDeliveryChanged {
                    state: AgentOperationDelivery::DispatchFenced,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn parent_retirement_between_policy_read_and_commit_rejects_without_input_or_agent_mutation()
{
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        changing_registry(store.clone(), sender, ParentChange::Retire, 1),
    );
    assert!(matches!(
        service
            .send_message(&owner(), &ready.id, "Too late".into())
            .await,
        Err(AgentOperationError::StatusConflict { .. })
    ));
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    assert!(store.agent_log(&ready.id, 0, 100).await.unwrap().is_empty());
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn already_admitted_operation_settlement_and_runtime_output_survive_parent_pause() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = changing_registry(store.clone(), sender, ParentChange::Pause, 2);
    let service = AgentOperationService::new(store.clone(), registry);
    let AgentMessageOutcome::Dispatched(receipt) = service
        .send_message(&owner(), &ready.id, "Finish admitted work".into())
        .await
        .unwrap()
    else {
        panic!("request was not admitted");
    };
    assert_eq!(receipt.delivery, AgentOperationDelivery::WriteConfirmed);
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let result = recorder
        .record_update(
            &ready.id,
            3,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            crate::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(result.turn_closed);
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
    assert_eq!(
        store
            .get_sandbox(&ready.sandbox_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Pausing
    );
}

#[tokio::test]
async fn direct_store_admission_requires_exact_parent_even_when_agent_revision_is_unchanged() {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = seed_active_parent(&store).await;
    let ready = test_agent(AgentStatus::Ready, None);
    store.create_agent(ready.clone()).await.unwrap();
    let mut queued = ready.clone();
    queued.revision += 1;
    queued.conversation.last_seq = 1;
    queued.updated_at = LATER.into();
    queued
        .conversation
        .queued_messages
        .push(QueuedAgentMessage::new(1, "Accepted owner work".into(), LATER.into()).unwrap());
    let mut mutation = AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        reconciliation_fence: None,
        expected_agent_revision: ready.revision,
        expected_last_seq: 0,
        runtime_generation: 3,
        agent: queued,
        appended_at: LATER.into(),
        agent_logs: vec![AgentLogKind::OwnerMessage {
            content: "Accepted owner work".into(),
        }],
    };
    assert_eq!(
        store.commit_agent_conversation(mutation.clone()).await,
        Err(store::SandboxStoreError::Conflict)
    );
    mutation.admission_guard = Some(SandboxMutationGuard::capture(&parent));
    let mut changed = parent.clone();
    changed.begin_pause(LATER.into(), None).unwrap();
    store
        .save_sandbox(parent.revision, changed.clone())
        .await
        .unwrap();
    assert_eq!(
        store.commit_agent_conversation(mutation.clone()).await,
        Err(store::SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&ready.id).await.unwrap(),
        Some(ready.clone())
    );
    assert!(store.agent_log(&ready.id, 0, 100).await.unwrap().is_empty());
    mutation.admission_guard = Some(SandboxMutationGuard::capture(&changed));
    store.commit_agent_conversation(mutation).await.unwrap();
    assert_eq!(
        store
            .get_agent(&ready.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .queued_messages
            .len(),
        1
    );
}

fn changing_registry(
    store: Arc<InMemorySandboxStore>,
    sender: ConversationInputSender,
    change: ParentChange,
    change_at: usize,
) -> Arc<ChangingParentRegistry> {
    Arc::new(ChangingParentRegistry {
        store,
        sender,
        resolution: AtomicUsize::new(0),
        change_at,
        change,
    })
}
