use std::collections::HashMap;

use layer2_core::now_rfc3339;
use layer2_core::sandbox::{AgentMessageAssembly, AgentRuntimeConnection, AgentUpdateRecorder};
use layer3_adapter::{
    ContentBlock, ContentChunk, MessageId, PermissionDecision, PermissionLocator, PermissionOption,
    PermissionOptionKind, PermissionRequest, PromptResponse, RequestPermissionRequest,
    SessionUpdate, ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    TurnStopReason,
};

use super::*;

pub(super) const APPROVAL_ID: &str = "sdk-read-file";
pub(super) const APPROVAL_LOCATOR: &str = "private-sdk-permission-route";

pub(super) struct SdkRegistry {
    live: Arc<LiveAgentRegistry>,
    recorder: Arc<DurableAgentUpdateRecorder>,
    inputs: Mutex<HashMap<(AgentId, u64), ConversationInputSender>>,
    probe: Option<Arc<RuntimeProbe>>,
}

/// Counts every input at the runtime boundary, without deduplication. Output
/// waits for an explicit release after the test replaces its command worker.
pub(super) struct RuntimeProbe {
    inputs: Mutex<Vec<(AgentRuntimeTarget, ConversationInput)>>,
    replies: tokio::sync::Semaphore,
    outcome: RuntimeOutcome,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeOutcome {
    Completed,
    Failed,
    Interrupted,
    Approval,
}

impl RuntimeProbe {
    pub(super) fn gated() -> Arc<Self> {
        Self::with_outcome(RuntimeOutcome::Completed)
    }

    pub(super) fn failing() -> Arc<Self> {
        Self::with_outcome(RuntimeOutcome::Failed)
    }

    pub(super) fn interruptible() -> Arc<Self> {
        Self::with_outcome(RuntimeOutcome::Interrupted)
    }

    pub(super) fn approving() -> Arc<Self> {
        Self::with_outcome(RuntimeOutcome::Approval)
    }

    fn with_outcome(outcome: RuntimeOutcome) -> Arc<Self> {
        Arc::new(Self {
            inputs: Mutex::new(Vec::new()),
            replies: tokio::sync::Semaphore::new(0),
            outcome,
        })
    }

    pub(super) fn inputs(&self) -> Vec<(AgentRuntimeTarget, ConversationInput)> {
        self.inputs.lock().unwrap().clone()
    }

    pub(super) fn release_reply(&self) {
        self.replies.add_permits(1);
    }
}

impl SdkRegistry {
    pub(super) fn new(
        live: Arc<LiveAgentRegistry>,
        recorder: Arc<DurableAgentUpdateRecorder>,
    ) -> Arc<Self> {
        Self::with_probe(live, recorder, None)
    }

    pub(super) fn with_probe(
        live: Arc<LiveAgentRegistry>,
        recorder: Arc<DurableAgentUpdateRecorder>,
        probe: Option<Arc<RuntimeProbe>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            live,
            recorder,
            inputs: Mutex::new(HashMap::new()),
            probe,
        })
    }
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for SdkRegistry {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let connection = AgentRuntimeRegistry::resolve(self.live.as_ref(), target).await?;
        let mut inputs = self.inputs.lock().unwrap();
        let input = inputs.entry((target.agent_id.clone(), target.runtime_generation)).or_insert_with(|| {
            let target = target.clone();
            let recorder = self.recorder.clone();
            let probe = self.probe.clone();
            let owner = layer2_core::agent_ownership::execution_authority();
            ConversationInputSender::spawn(4, move |input| {
                let target = target.clone();
                let recorder = recorder.clone();
                let probe = probe.clone();
                let owner = owner.clone();
                async move {
                    if let Some(probe) = &probe {
                        probe.inputs.lock().unwrap().push((target.clone(), input.clone()));
                    }
                    let outcome = probe.as_ref().map_or(RuntimeOutcome::Completed, |probe| probe.outcome);
                    let ask_approval = outcome == RuntimeOutcome::Approval && matches!(&input, ConversationInput::Prompt(_) | ConversationInput::RecoverablePrompt { .. });
                    let with_tools = match input {
                        ConversationInput::Prompt(_) | ConversationInput::RecoverablePrompt { .. } if outcome == RuntimeOutcome::Interrupted => return Ok(()),
                        ConversationInput::Prompt(content) | ConversationInput::RecoverablePrompt { content, .. } => content.iter().any(|block| matches!(block, ContentBlock::Text(text) if text.text.lines().next() == Some("sdk-raw-with-tools"))),
                        ConversationInput::Cancel if outcome == RuntimeOutcome::Interrupted => false,
                        ConversationInput::PermissionAnswer { id, locator, decision }
                            if outcome == RuntimeOutcome::Approval && id == APPROVAL_ID
                                && locator.as_ref().map(PermissionLocator::as_str) == Some(APPROVAL_LOCATOR)
                                && decision == PermissionDecision::Allow => false,
                        _ => return Err(AdapterError::Unsupported("SDK fixture input is not supported".into())),
                    };
                    tokio::spawn(async move {
                        if let Some(probe) = probe {
                            probe.replies.acquire().await.unwrap().forget();
                        }
                        let record = async move {
                            if ask_approval {
                                emit_approval(recorder, target).await;
                            } else {
                                emit_reply(recorder, target, with_tools, outcome).await;
                            }
                        };
                        if let Some(owner) = owner {
                            owner.run(record).await.unwrap();
                        } else {
                            record.await;
                        }
                    });
                    Ok(())
                }
            })
        }).clone();
        Ok(AgentRuntimeConnection {
            capabilities: connection.capabilities,
            input,
        })
    }

    async fn recover_missing_attachment(&self, target: &AgentRuntimeTarget) {
        AgentRuntimeRegistry::recover_missing_attachment(self.live.as_ref(), target).await;
    }

    async fn fence(
        &self,
        agent_id: &AgentId,
        authority_fence: u64,
    ) -> Result<(), AgentRuntimeError> {
        AgentRuntimeRegistry::fence(self.live.as_ref(), agent_id, authority_fence).await
    }
}

async fn emit_approval(recorder: Arc<DurableAgentUpdateRecorder>, target: AgentRuntimeTarget) {
    recorder
        .record_update(
            &target.agent_id,
            target.runtime_generation,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Asked(PermissionRequest {
                id: APPROVAL_ID.into(),
                locator: Some(PermissionLocator::new(APPROVAL_LOCATOR)),
                request: RequestPermissionRequest::new(
                    target.conversation_id,
                    ToolCallUpdate::new("sdk-read", ToolCallUpdateFields::new().title("Read file")),
                    vec![
                        PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                        PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
                    ],
                ),
            }),
            now_rfc3339(),
        )
        .await
        .unwrap();
}

async fn emit_reply(
    recorder: Arc<DurableAgentUpdateRecorder>,
    target: AgentRuntimeTarget,
    with_tools: bool,
    outcome: RuntimeOutcome,
) {
    let mut updates = vec![ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(
        ContentChunk::new(ContentBlock::from("Checking the proof")),
    ))];
    if with_tools {
        updates.push(ConversationUpdate::Said(SessionUpdate::ToolCall(
            ToolCall::new("sdk-proof", "Check proof")
                .kind(ToolKind::Execute)
                .status(ToolCallStatus::InProgress),
        )));
        updates.push(ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(
                "sdk-proof",
                ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
            ),
        )));
    }
    let text = match outcome {
        RuntimeOutcome::Completed | RuntimeOutcome::Approval => ["Hello ", "from CMA"],
        RuntimeOutcome::Failed => ["Partial output ", "before failure"],
        RuntimeOutcome::Interrupted => ["Partial output ", "before cancellation"],
    };
    for text in text {
        updates.push(ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from(text)).message_id(MessageId::new("sdk-answer")),
        )));
    }
    let mut assembly = AgentMessageAssembly::default();
    for update in updates {
        tokio::time::sleep(Duration::from_millis(100)).await;
        recorder
            .record_update(
                &target.agent_id,
                target.runtime_generation,
                &mut assembly,
                update,
                now_rfc3339(),
            )
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    let terminal = match outcome {
        RuntimeOutcome::Failed => ConversationUpdate::Failed {
            message: "The runtime failed after producing partial output.".into(),
            capability: Some("fixture_turn_failed".into()),
        },
        RuntimeOutcome::Completed | RuntimeOutcome::Approval => {
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn))
        }
        RuntimeOutcome::Interrupted => {
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::Cancelled))
        }
    };
    recorder
        .record_update(
            &target.agent_id,
            target.runtime_generation,
            &mut assembly,
            terminal,
            now_rfc3339(),
        )
        .await
        .unwrap();
}
