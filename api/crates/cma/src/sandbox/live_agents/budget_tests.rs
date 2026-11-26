use super::*;
use layer2_core::budget::BudgetError;
use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome};
use layer2_core::AgentId;
use layer3_adapter::{ConversationSendOutcome, ConversationUpdate};
use std::sync::Mutex;

struct Recorder(Mutex<Result<Option<BudgetStop>, AgentConversationError>>);
#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("sending cancellation is not terminal evidence")
    }
    async fn budget_stop(
        &self,
        _: &AgentId,
        _: u64,
    ) -> Result<Option<BudgetStop>, AgentConversationError> {
        self.0.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn budget_cancel_requires_current_attachment_and_rechecked_turn_before_write() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let attachment = LiveAgentAttachmentIdentity {
        agent_id: AgentId("budget-agent".into()),
        conversation_id: "budget-conversation".into(),
        runtime_generation: 1,
        authority_fence: 1,
        nonce: 1,
    };
    let stop = BudgetStop {
        agent_id: attachment.agent_id.clone(),
        runtime_generation: 1,
        turn_sequence: 1,
        reason: BudgetError::TurnExhausted,
    };
    let recorder = Arc::new(Recorder(Mutex::new(Ok(Some(stop.clone())))));
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let input = ConversationInputSender::spawn(4, move |message| {
        sent.send(message).unwrap();
        async { Ok(()) }
    });
    let task = tokio::spawn(std::future::pending::<()>());
    registry.lock().entries.insert(
        attachment.agent_id.clone(),
        super::super::RegistryEntry {
            conversation_id: attachment.conversation_id.clone(),
            runtime_generation: 1,
            authority_fence: 1,
            manager_authority: None,
            capabilities: layer2_core::RuntimeCapabilities::default(),
            input: super::super::AttachmentInput::Connected {
                input: input.clone(),
                activation: None,
            },
            published: true,
            nonce: 1,
            task: task.abort_handle(),
        },
    );
    let guard = Arc::new(BudgetInterruptGuard {
        registry: Arc::downgrade(&registry),
        attachment: attachment.clone(),
        recorder: recorder.clone(),
        stop: stop.clone(),
    });
    assert!(matches!(
        input
            .send_guarded(ConversationInput::Cancel, guard.clone())
            .await,
        ConversationSendOutcome::WriteConfirmed
    ));
    assert!(matches!(
        received.try_recv().unwrap(),
        ConversationInput::Cancel
    ));
    for stale in ["turn", "scope", "source", "attachment"] {
        *recorder.0.lock().unwrap() = Ok(Some(stop.clone()));
        match stale {
            "turn" => {
                recorder
                    .0
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .turn_sequence += 1
            }
            "scope" => {
                recorder
                    .0
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .agent_id = AgentId("other-agent".into())
            }
            "source" => {
                *recorder.0.lock().unwrap() =
                    Err(AgentConversationError::Unavailable("meter offline".into()))
            }
            "attachment" => {
                registry
                    .lock()
                    .entries
                    .get_mut(&attachment.agent_id)
                    .unwrap()
                    .nonce += 1
            }
            _ => unreachable!(),
        }
        assert!(!matches!(
            input
                .send_guarded(ConversationInput::Cancel, guard.clone())
                .await,
            ConversationSendOutcome::WriteConfirmed
        ));
        assert!(received.try_recv().is_err());
    }
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn exhausted_recovery_waits_for_publication_before_interrupting() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let target = super::super::LiveAgentTarget {
        agent_id: AgentId("budget-recovery".into()),
        conversation_id: "retained".into(),
        runtime_generation: 1,
    };
    let stop = BudgetStop {
        agent_id: target.agent_id.clone(),
        runtime_generation: 1,
        turn_sequence: 3,
        reason: BudgetError::TurnExhausted,
    };
    let recorder = Arc::new(Recorder(Mutex::new(Ok(Some(stop)))));
    let (_send, received) = tokio::sync::mpsc::channel(1);
    let recovery = registry
        .begin_recovery(
            &target,
            2,
            layer2_core::RuntimeCapabilities::default(),
            received,
            recorder,
        )
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    let (cancel, mut cancelled) = tokio::sync::mpsc::channel(1);
    let input = ConversationInputSender::spawn(1, move |input| {
        let cancel = cancel.clone();
        async move {
            assert!(matches!(input, ConversationInput::Cancel));
            cancel.send(()).await.unwrap();
            Ok(())
        }
    });
    let attachment = recovery.finish(input).unwrap();
    registry.publish_and_activate(&attachment).unwrap();
    tokio::time::timeout(Duration::from_secs(3), cancelled.recv())
        .await
        .unwrap()
        .unwrap();
    registry.fence(&target.agent_id, Some(1), 2);
}
