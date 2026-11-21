use std::sync::Arc;

use layer3_adapter::{ConversationInput, ConversationUpdate, PromptResponse, TurnStopReason};

use crate::sandbox::store::{AgentLogStore, AgentStore, SandboxStore, SandboxWorkerStore};
use crate::sandbox::{
    AgentMessageAssembly, AgentMessageOutcome, AgentOperationDelivery, AgentOperationError,
    AgentOperationService, AgentStatus, AgentTaskStatus, AgentUpdateRecorder,
    DurableAgentUpdateRecorder, InMemorySandboxStore, QueuedMessageState,
    SandboxImageRotationPhase, SandboxReviveCause, SandboxRevivePhase, SandboxRotationIdentity,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

use super::fixtures::*;

const LEASE_END: &str = "2026-09-06T00:00:00Z";
const BEFORE_GRACE: &str = "2026-09-05T00:14:59Z";
const BEFORE_GRACE_LEASE_END: &str = "2026-09-05T00:15:00Z";
const GRACE_ELAPSED: &str = "2026-09-05T00:15:01Z";
const RECOVERY_RETRY: &str = "2026-09-05T00:15:31Z";

#[tokio::test]
async fn lifecycle_queue_fences_exactly_once_without_relogging_owner_input() {
    let store = Arc::new(InMemorySandboxStore::default());
    let sandbox = seed_queue_sandbox(&store).await;
    let agent = test_agent(AgentStatus::Ready, None);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    let first = queue(&service, &agent, "first queued message").await;
    queue(&service, &agent, "second queued message").await;

    let receipt = service
        .dispatch_lifecycle_queued_message(&sandbox, &agent.id, first, LATER.into())
        .await
        .unwrap();

    assert_eq!(receipt.delivery, AgentOperationDelivery::WriteConfirmed);
    assert_eq!(receipt.agent.status, AgentStatus::Working);
    assert_eq!(
        receipt
            .agent
            .conversation
            .queued_messages
            .iter()
            .map(|message| message.state)
            .collect::<Vec<_>>(),
        [QueuedMessageState::Delivered, QueuedMessageState::Queued]
    );
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [ConversationInput::RecoverablePrompt { content, .. }] if prompt_text(content) == "first queued message"
    ));
    let owner_messages = store
        .agent_log(&agent.id, 0, 20)
        .await
        .unwrap()
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.kind,
                crate::sandbox::AgentLogKind::OwnerMessage { .. }
            )
        })
        .count();
    assert_eq!(owner_messages, 2);
    let progressed = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert!(progressed.work.lease.is_none());
    assert_eq!(progressed.lifecycle.operation, sandbox.lifecycle.operation);
    assert_eq!(registry.resolutions(), 1);
}

#[tokio::test]
async fn lifecycle_queue_pre_write_failure_requeues_the_same_item() {
    let store = Arc::new(InMemorySandboxStore::default());
    let sandbox = seed_queue_sandbox(&store).await;
    let agent = test_agent(AgentStatus::Ready, None);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(1);
    let service = AgentOperationService::new(store.clone(), registry);
    let sequence = queue(&service, &agent, "retry me").await;

    assert!(matches!(
        service
            .dispatch_lifecycle_queued_message(&sandbox, &agent.id, sequence, LATER.into())
            .await,
        Err(AgentOperationError::RuntimeUnavailable(_))
    ));

    let stored = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
    assert_eq!(
        stored.conversation.queued_messages[0].state,
        QueuedMessageState::Queued
    );
    assert_eq!(
        stored.conversation.operation.unwrap().state,
        AgentOperationDelivery::PreWriteFailed
    );
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn lifecycle_queue_ambiguous_write_waits_for_execution_evidence() {
    let store = Arc::new(InMemorySandboxStore::default());
    let sandbox = seed_queue_sandbox(&store).await;
    let agent = test_agent(AgentStatus::Ready, None);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Err(layer3_adapter::AdapterError::Rejected(
        "receipt disappeared".into(),
    )));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry);
    let sequence = queue(&service, &agent, "only once").await;

    assert!(matches!(
        service
            .dispatch_lifecycle_queued_message(&sandbox, &agent.id, sequence, LATER.into())
            .await,
        Err(AgentOperationError::DeliveryAmbiguous { .. })
    ));
    let uncertain = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        uncertain.conversation.queued_messages[0].state,
        QueuedMessageState::RecoveryRequired
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);

    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            3,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            LATEST.into(),
        )
        .await
        .unwrap();

    let observed = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(observed.status, AgentStatus::Ready);
    assert_eq!(
        observed.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::ExecutionObserved
    );
    assert_eq!(
        observed.conversation.queued_messages[0].state,
        QueuedMessageState::Delivered
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        AgentTaskStatus::from_log(&observed, &history, false),
        AgentTaskStatus::Completed
    );
    assert_eq!(
        AgentTaskStatus::from_log(&observed, &history, true),
        AgentTaskStatus::Working
    );
    for state in [
        QueuedMessageState::Queued,
        QueuedMessageState::DispatchFenced,
        QueuedMessageState::RecoveryRequired,
    ] {
        let mut outstanding = observed.clone();
        outstanding.conversation.queued_messages[0].state = state;
        assert_eq!(
            AgentTaskStatus::from_log(&outstanding, &history, false),
            AgentTaskStatus::Working,
            "outstanding {state:?} must block completion"
        );
    }
    let mut active = observed.clone();
    active.conversation.active_turn = Some(100);
    assert_eq!(
        AgentTaskStatus::from_log(&active, &history, false),
        AgentTaskStatus::Working
    );
}

#[tokio::test]
async fn lifecycle_queue_rejects_a_non_fifo_sequence_before_runtime_input() {
    let store = Arc::new(InMemorySandboxStore::default());
    let sandbox = seed_queue_sandbox(&store).await;
    let agent = test_agent(AgentStatus::Ready, None);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    queue(&service, &agent, "first").await;
    let second = queue(&service, &agent, "second").await;
    let before = store.get_agent(&agent.id).await.unwrap().unwrap();

    assert_eq!(
        service
            .dispatch_lifecycle_queued_message(&sandbox, &agent.id, second, LATER.into())
            .await,
        Err(AgentOperationError::Conflict)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(before));
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(registry.resolutions(), 0);
}

#[tokio::test]
async fn image_rotation_queues_input_while_an_agent_is_transiently_ready() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = test_agent(AgentStatus::Ready, None);
    let sandbox = seed_rotation_queue_sandbox(&store, &agent).await;
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    let outcome = service
        .send_message(&owner(), &agent.id, "wait for rotation".into())
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        AgentMessageOutcome::Queued { sequence: 1, .. }
    ));
    let stored = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.queued_messages.len(), 1);
    assert_eq!(
        stored.conversation.queued_messages[0].state,
        QueuedMessageState::Queued
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(registry.resolutions(), 0);
    assert_eq!(
        sandbox.status,
        SandboxStatus::Rotating,
        "the fixture must exercise rotation routing"
    );
}

#[tokio::test]
async fn abandoned_queue_fence_requires_grace_then_enters_recovery_without_resend() {
    let store = Arc::new(InMemorySandboxStore::default());
    let sandbox = seed_queue_sandbox(&store).await;
    let agent = test_agent(AgentStatus::Ready, None);
    store.create_agent(agent.clone()).await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let inputs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let sender = layer3_adapter::ConversationInputSender::spawn(2, {
        let entered = entered.clone();
        let release = release.clone();
        let inputs = inputs.clone();
        move |_| {
            let entered = entered.clone();
            let release = release.clone();
            let inputs = inputs.clone();
            async move {
                inputs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                entered.notify_one();
                release.notified().await;
                Ok(())
            }
        }
    });
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());
    let sequence = queue(&service, &agent, "write once").await;
    let dispatch = tokio::spawn({
        let service = service.clone();
        let sandbox = sandbox.clone();
        let agent_id = agent.id.clone();
        async move {
            service
                .dispatch_lifecycle_queued_message(&sandbox, &agent_id, sequence, LATER.into())
                .await
        }
    });
    entered.notified().await;
    dispatch.abort();
    assert!(dispatch.await.unwrap_err().is_cancelled());
    release.notify_one();
    tokio::task::yield_now().await;

    let fenced = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        fenced.conversation.queued_messages[0].state,
        QueuedMessageState::DispatchFenced
    );
    let progressed = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    let early = store
        .claim_sandbox_work(
            &sandbox.id,
            "early-worker",
            BEFORE_GRACE.into(),
            BEFORE_GRACE_LEASE_END.into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        service
            .recover_stale_lifecycle_queue_dispatch(
                &early,
                &agent.id,
                sequence,
                RECOVERY_RETRY.into(),
                BEFORE_GRACE.into(),
            )
            .await,
        Err(AgentOperationError::Conflict)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), fenced);

    let stale = store
        .claim_sandbox_work(
            &sandbox.id,
            "recovery-worker",
            GRACE_ELAPSED.into(),
            LEASE_END.into(),
        )
        .await
        .unwrap()
        .unwrap();
    service
        .recover_stale_lifecycle_queue_dispatch(
            &stale,
            &agent.id,
            sequence,
            RECOVERY_RETRY.into(),
            GRACE_ELAPSED.into(),
        )
        .await
        .unwrap();

    let recovered = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, AgentStatus::Working);
    assert_eq!(
        recovered.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::RecoveryRequired
    );
    assert_eq!(
        recovered.conversation.queued_messages[0].state,
        QueuedMessageState::RecoveryRequired
    );
    let retried = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(retried.work.ready_at, RECOVERY_RETRY);
    assert_eq!(retried.work.attempts, progressed.work.attempts + 1);
    assert!(retried.work.lease.is_none());
    assert_eq!(inputs.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(registry.resolutions(), 1);
}

async fn queue(service: &AgentOperationService, agent: &crate::sandbox::Agent, text: &str) -> u64 {
    match service
        .send_message(&owner(), &agent.id, text.into())
        .await
        .unwrap()
    {
        AgentMessageOutcome::Queued { sequence, .. } => sequence,
        AgentMessageOutcome::Dispatched(_) => panic!("lifecycle input bypassed the queue"),
    }
}

async fn seed_queue_sandbox(store: &InMemorySandboxStore) -> crate::sandbox::Sandbox {
    let mut sandbox = crate::sandbox::lifecycle_test_support::sandbox();
    sandbox.id = crate::SandboxId("sbx_operations".into());
    let runtime = sandbox.runtime.take().unwrap();
    sandbox
        .lifecycle
        .checkpoints
        .publish(crate::sandbox::lifecycle_test_support::checkpoint(
            1,
            crate::sandbox::lifecycle_test_support::NOW,
        ))
        .unwrap();
    sandbox.status = SandboxStatus::Paused;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    sandbox
        .begin_revive(SandboxReviveCause::AgentMessage, NOW.into())
        .unwrap();
    let Some(crate::sandbox::SandboxLifecycleOperation::Revive { phase, .. }) =
        sandbox.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *phase = SandboxRevivePhase::QueuedMessageDelivery;
    sandbox.runtime = Some(runtime);
    sandbox
        .work
        .claim("lifecycle-worker".into(), NOW, LEASE_END.into())
        .unwrap();
    sandbox.revision = 1;
    sandbox.validate().unwrap();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    sandbox
}

async fn seed_rotation_queue_sandbox(
    store: &InMemorySandboxStore,
    agent: &crate::sandbox::Agent,
) -> crate::sandbox::Sandbox {
    let mut sandbox = crate::sandbox::lifecycle_test_support::sandbox();
    sandbox.id = crate::SandboxId("sbx_operations".into());
    sandbox
        .lifecycle
        .checkpoints
        .publish(crate::sandbox::lifecycle_test_support::checkpoint(
            1,
            crate::sandbox::lifecycle_test_support::NOW,
        ))
        .unwrap();
    let identity = SandboxRotationIdentity::capture(&sandbox, std::slice::from_ref(agent)).unwrap();
    sandbox
        .begin_image_rotation("image-v2".into(), identity, NOW.into())
        .unwrap();
    let Some(crate::sandbox::SandboxLifecycleOperation::ImageRotation {
        phase,
        checkpoint_epoch,
        ..
    }) = sandbox.lifecycle.operation.as_mut()
    else {
        unreachable!()
    };
    *phase = SandboxImageRotationPhase::QueuedMessageDelivery;
    *checkpoint_epoch = Some(1);
    sandbox.runtime.as_mut().unwrap().runtime_version = "image-v2".into();
    sandbox.revision = 1;
    sandbox.validate().unwrap();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    sandbox
}
