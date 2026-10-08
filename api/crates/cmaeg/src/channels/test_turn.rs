//! Bounded synthetic turn used to verify a channel's provider binding.
use super::management::{load, ManagementError, ManagementState};
use super::target::ChannelTarget;
use crate::{
    auth::Creator,
    providers::{ProviderError, ResponsesEvent, TurnRequest},
};
use cmaeg_core::{
    channel::ConformanceLevel,
    ids::{ConsumerId, ConversationId, IdempotencyKey},
};
use std::{collections::BTreeMap, time::Duration};

#[derive(Debug, serde::Serialize)]
pub struct TestResult {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: ConformanceLevel,
}

pub async fn test_turn(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
) -> Result<TestResult, ManagementError> {
    test_consumer_turn(state, creator, target, None).await
}

pub async fn test_consumer_turn(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    consumer: Option<&ConsumerId>,
) -> Result<TestResult, ManagementError> {
    tokio::time::timeout(
        Duration::from_secs(30),
        run(state, creator, target, consumer),
    )
    .await
    .map_err(|_| unavailable("test turn timed out"))?
}

async fn run(
    state: &ManagementState,
    creator: &Creator,
    target: &ChannelTarget,
    id: Option<&ConsumerId>,
) -> Result<TestResult, ManagementError> {
    let current = load(state, creator, target).await?;
    let definition = current.definition;
    if id.is_some_and(|id| !id.is_default()) && !state.routing.multi_consumer_enabled {
        return Err(ManagementError::InvalidReason(
            super::write_gate::MULTI_CONSUMER_DISABLED,
        ));
    }
    let consumer = definition.consumer(id).ok_or(ManagementError::NotFound)?;
    let provider = consumer.provider;
    let credential = consumer
        .credential
        .ok_or_else(|| unavailable("channel provider credential is unavailable"))?;
    let conversation_id = if consumer.id.is_default() {
        ConversationId::derive(
            &state.master_keys,
            &current.channel_id,
            definition.partition.version,
            "test",
        )
    } else {
        ConversationId::derive_for_consumer(
            &state.master_keys,
            &current.channel_id,
            definition.partition.version,
            consumer.id,
            "test",
        )
    };
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
    let mut stream = state.providers.start_turn(&provider.slug, TurnRequest {
                timing: None,
        conversation_id,
        input: vec![serde_json::json!({"role":"user","content":[{"type":"input_text","text":"CMAEG channel test"}]})],
        idempotency_key: IdempotencyKey::new(format!("test_{}", uuid::Uuid::new_v4().simple()))
            .map_err(ManagementError::Invalid)?,
        metadata: BTreeMap::new(), agent_key: credential.clone(), event_context: None,
    }).await.map_err(ManagementError::Provider)?;
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
