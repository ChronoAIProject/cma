//! Bounded synthetic turn used to verify a channel's provider binding.
use super::management::{load, ManagementError, ManagementState};
use crate::{
    auth::Creator,
    providers::{ProviderError, ResponsesEvent, TurnRequest},
};
use cmaeg_core::{
    channel::ConformanceLevel,
    ids::{ChannelId, ConversationId, IdempotencyKey},
};
use std::{collections::BTreeMap, time::Duration};

const MIN_BUSY_RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, serde::Serialize)]
pub struct TestResult {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: ConformanceLevel,
}

pub async fn test_turn(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
) -> Result<TestResult, ManagementError> {
    tokio::time::timeout(Duration::from_secs(30), run(state, creator, channel_id))
        .await
        .map_err(|_| unavailable("test turn timed out"))?
}

async fn run(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
) -> Result<TestResult, ManagementError> {
    let current = load(state, creator, channel_id).await?;
    let definition = current.definition;
    // The test turn verifies the binding management created: the default
    // consumer's.
    let consumer = definition
        .consumer(None)
        .ok_or_else(|| unavailable("channel consumer is not configured"))?;
    let provider = consumer.provider;
    let credential = consumer
        .credential
        .ok_or_else(|| unavailable("channel provider credential is unavailable"))?;
    let conversation_id = ConversationId::derive(
        &state.master_keys,
        channel_id,
        definition.partition.version,
        "test",
    );
    state
        .providers
        .ensure_conversation(
            &provider.slug,
            &provider.binding_id,
            &conversation_id,
            credential,
        )
        .await
        .map_err(ManagementError::Provider)?;
    let request = TurnRequest {
        timing: None,
        conversation_id,
        input: vec![
            serde_json::json!({"role":"user","content":[{"type":"input_text","text":"CMAEG channel test"}]}),
        ],
        idempotency_key: IdempotencyKey::new(format!("test_{}", uuid::Uuid::new_v4().simple()))
            .map_err(ManagementError::Invalid)?,
        metadata: BTreeMap::new(),
        agent_key: credential.clone(),
        event_context: None,
    };
    let mut stream = loop {
        match state
            .providers
            .start_turn(&provider.slug, request.clone())
            .await
        {
            Ok(stream) => break stream,
            Err(ProviderError::Busy { retry_after }) => {
                // Only an explicit refusal permits replay. Keep the same key and
                // body while the outer deadline also bounds this readiness wait.
                tokio::time::sleep(
                    retry_after
                        .unwrap_or(MIN_BUSY_RETRY_DELAY)
                        .max(MIN_BUSY_RETRY_DELAY),
                )
                .await;
            }
            Err(error) => return Err(ManagementError::Provider(error)),
        }
    };
    let accepted = stream
        .accepted
        .await
        .map_err(|_| unavailable("test turn was not accepted"))?;
    let mut first_text = String::new();
    let mut remaining = 200;
    let mut completed = false;
    while let Some(event) = stream.events.recv().await {
        match event {
            ResponsesEvent::OutputTextDelta { delta, .. } => {
                let piece: String = delta.chars().take(remaining).collect();
                remaining -= piece.chars().count();
                first_text.push_str(&piece);
            }
            ResponsesEvent::Completed { .. } => {
                completed = true;
                break;
            }
            ResponsesEvent::Failed { .. } | ResponsesEvent::Incomplete { .. } => {
                return Err(unavailable("provider test did not complete"))
            }
            _ => {}
        }
    }
    if !completed {
        return Err(unavailable("provider test stream ended before completion"));
    }
    Ok(TestResult {
        accepted_after_ms: accepted
            .at
            .duration_since(stream.request_started)
            .as_millis() as u64,
        first_text,
        level: provider.level,
    })
}

fn unavailable(message: &str) -> ManagementError {
    ManagementError::Provider(ProviderError::Unavailable(message.into()))
}
