use std::collections::HashMap;

use layer2_core::now_rfc3339;
use layer2_core::sandbox::{AgentMessageAssembly, AgentRuntimeConnection, AgentUpdateRecorder};
use layer3_adapter::{
    ContentBlock, ContentChunk, MessageId, PromptResponse, SessionUpdate, ToolCall, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, TurnStopReason,
};

use super::*;

pub(super) struct SdkRegistry {
    live: Arc<LiveAgentRegistry>,
    recorder: Arc<DurableAgentUpdateRecorder>,
    inputs: Mutex<HashMap<(AgentId, u64), ConversationInputSender>>,
    probe: Option<Arc<RuntimeProbe>>,
}

/// Counts every input at the runtime boundary, without deduplication. Output
/// waits for the test to disconnect its reader and replace its command worker.
pub(super) struct RuntimeProbe {
    inputs: Mutex<Vec<(AgentRuntimeTarget, ConversationInput)>>,
    replies: tokio::sync::Semaphore,
}

impl RuntimeProbe {
    pub(super) fn gated() -> Arc<Self> {
        Arc::new(Self {
            inputs: Mutex::new(Vec::new()),
            replies: tokio::sync::Semaphore::new(0),
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
            ConversationInputSender::spawn(4, move |input| {
                let target = target.clone();
                let recorder = recorder.clone();
                let probe = probe.clone();
                async move {
                    if let Some(probe) = &probe {
                        probe.inputs.lock().unwrap().push((target.clone(), input.clone()));
                    }
                    let ConversationInput::Prompt(content) = input else {
                        return Err(AdapterError::Unsupported("SDK fixture accepts only prompts".into()));
                    };
                    let with_tools = content.iter().any(|block| matches!(block, ContentBlock::Text(text) if text.text == "sdk-raw-with-tools"));
                    tokio::spawn(async move {
                        if let Some(probe) = probe {
                            probe.replies.acquire().await.unwrap().forget();
                        }
                        emit_reply(recorder, target, with_tools).await;
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

    async fn fence(
        &self,
        agent_id: &AgentId,
        authority_fence: u64,
    ) -> Result<(), AgentRuntimeError> {
        AgentRuntimeRegistry::fence(self.live.as_ref(), agent_id, authority_fence).await
    }
}

async fn emit_reply(
    recorder: Arc<DurableAgentUpdateRecorder>,
    target: AgentRuntimeTarget,
    with_tools: bool,
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
    for text in ["Hello ", "from CMA"] {
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
    recorder
        .record_update(
            &target.agent_id,
            target.runtime_generation,
            &mut assembly,
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            now_rfc3339(),
        )
        .await
        .unwrap();
}
