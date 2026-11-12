//! Bounded synthetic turn used to verify a channel's provider binding.
use super::management::{load, ManagementError, ManagementState};
use crate::{
    auth::Creator,
    providers::{ProviderError, ResponsesEvent, TurnRequest, TurnStream},
};
use cmaeg_core::{
    channel::ConformanceLevel,
    ids::{ChannelId, ConsumerId, ConversationId, IdempotencyKey},
};
use std::{collections::BTreeMap, time::Duration};

const MIN_BUSY_RETRY_DELAY: Duration = Duration::from_millis(500);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const TURN_TIMEOUT: Duration = Duration::from_secs(30);

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
    test_consumer(state, creator, channel_id, None).await
}

pub async fn test_consumer(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    consumer_id: Option<&ConsumerId>,
) -> Result<TestResult, ManagementError> {
    // A new profile binding may still be creating its Agent. Readiness has one
    // bounded allowance; it must not consume the admitted turn's output budget.
    let (stream, level) = tokio::time::timeout(
        STARTUP_TIMEOUT,
        start(state, creator, channel_id, consumer_id),
    )
    .await
    .map_err(|_| unavailable("test provider startup timed out"))??;
    tokio::time::timeout(TURN_TIMEOUT, complete(stream, level))
        .await
        .map_err(|_| unavailable("test turn timed out"))?
}

async fn start(
    state: &ManagementState,
    creator: &Creator,
    channel_id: &ChannelId,
    consumer_id: Option<&ConsumerId>,
) -> Result<(TurnStream, ConformanceLevel), ManagementError> {
    let current = load(state, creator, channel_id).await?;
    let definition = current.definition;
    let consumer = definition
        .consumer(consumer_id)
        .filter(|consumer| !consumer.pending)
        .ok_or(ManagementError::NotFound)?;
    let provider = consumer.provider;
    let credential = consumer
        .credential
        .ok_or_else(|| unavailable("channel provider credential is unavailable"))?;
    // Keep the legacy default test partition and isolate every additional
    // consumer's diagnostic conversation from it and ordinary user work.
    let partition = if consumer.id.is_default() {
        "test".into()
    } else {
        format!("test:consumer:{}", consumer.id)
    };
    let conversation_id = ConversationId::derive(
        &state.master_keys,
        channel_id,
        definition.partition.version,
        &partition,
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
        turn_trace: None,
    };
    let stream = loop {
        match state
            .providers
            .start_turn(&provider.slug, request.clone())
            .await
        {
            Ok(stream) => break stream,
            Err(ProviderError::Busy { retry_after }) => {
                // Only an explicit refusal permits replay. Keep the same key and
                // body while one startup deadline bounds every readiness wait.
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
    Ok((stream, provider.level))
}

async fn complete(
    mut stream: TurnStream,
    level: ConformanceLevel,
) -> Result<TestResult, ManagementError> {
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
        level,
    })
}

fn unavailable(message: &str) -> ManagementError {
    ManagementError::Provider(ProviderError::Unavailable(message.into()))
}
