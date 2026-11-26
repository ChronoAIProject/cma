//! Bounded synthetic turn used to verify a channel's provider binding.
use super::management::{load, ManagementError, ManagementState};
use crate::{
    auth::Creator,
    providers::{ProviderError, ResponsesEvent, TurnRequest, TurnStream},
    turns::{outcomes::TurnOutcome, Projection},
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
        nyxid_trace: state.providers.nyxid_trace_contract(provider),
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
    let mut projection = Projection::default();
    while let Some(frame) = stream.events.recv_observed().await {
        if let Some(first_text) = test_preview(
            &mut projection,
            frame.event,
            frame.bound_to_accepted,
            &accepted.response_id,
        )? {
            return Ok(TestResult {
                accepted_after_ms: accepted
                    .at
                    .duration_since(stream.request_started)
                    .as_millis() as u64,
                first_text,
                level,
            });
        }
    }
    Err(unavailable("provider test stream ended before completion"))
}

fn test_preview(
    projection: &mut Projection,
    event: ResponsesEvent,
    bound_to_accepted: bool,
    accepted_response: &str,
) -> Result<Option<String>, ManagementError> {
    if let ResponsesEvent::Completed {
        response,
        matching_response_identity,
    } = &event
    {
        if !bound_to_accepted
            || !matching_response_identity
            || response.get("id").and_then(serde_json::Value::as_str) != Some(accepted_response)
        {
            return Err(unavailable("provider test response could not be verified"));
        }
    }
    let outcome = projection
        .event(event)
        .map_err(|_| unavailable("provider test output could not be read"))?;
    match outcome {
        Some(TurnOutcome::Completed { final_text }) => Ok(Some(
            final_text.unwrap_or_default().chars().take(200).collect(),
        )),
        Some(TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. }) => {
            Err(unavailable("provider test did not complete"))
        }
        _ => Ok(None),
    }
}

fn unavailable(message: &str) -> ManagementError {
    ManagementError::Provider(ProviderError::Unavailable(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn completed(id: &str, matching_response_identity: bool) -> ResponsesEvent {
        ResponsesEvent::Completed {
            response: json!({
                "id": id,
                "output": [{ "type": "message", "content": [
                    { "type": "output_text", "text": "Channel test reply" }
                ] }],
            }),
            matching_response_identity,
        }
    }

    #[test]
    fn completed_test_uses_final_output_even_without_text_deltas() {
        let result = test_preview(
            &mut Projection::default(),
            completed("resp_test", true),
            true,
            "resp_test",
        )
        .unwrap();
        assert_eq!(result.as_deref(), Some("Channel test reply"));
    }

    #[test]
    fn test_refuses_completion_without_consistent_accepted_identity() {
        for (id, matching, bound) in [
            ("resp_other", true, true),
            ("resp_test", false, true),
            ("resp_test", true, false),
        ] {
            let result = test_preview(
                &mut Projection::default(),
                completed(id, matching),
                bound,
                "resp_test",
            );
            assert!(matches!(
                result,
                Err(ManagementError::Provider(ProviderError::Unavailable(reason)))
                    if reason == "provider test response could not be verified"
            ));
        }
    }

    #[test]
    fn test_uses_committed_text_and_caps_the_preview_by_character() {
        let mut projection = Projection::default();
        let result = test_preview(
            &mut projection,
            ResponsesEvent::OutputTextDone {
                item_id: "message".into(),
                text: "答".repeat(201),
            },
            true,
            "resp_test",
        )
        .unwrap();
        assert!(result.is_none());

        let result = test_preview(
            &mut projection,
            ResponsesEvent::Completed {
                response: json!({"id": "resp_test"}),
                matching_response_identity: true,
            },
            true,
            "resp_test",
        )
        .unwrap();
        assert_eq!(result, Some("答".repeat(200)));
    }
}
