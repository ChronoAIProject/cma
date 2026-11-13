//! Admit selected metadata/usage strings before creating an independent effect.

use std::sync::{Arc, OnceLock};

use serde_json::Value;
use tokio::sync::Semaphore;

use crate::ingress_budget::{IngressBudget, IngressReservation};
use crate::{
    AdapterError, ConversationDescriptionInput, ConversationDescriptionSource,
    ConversationMetadata, ConversationUpdate, ConversationUsage, RuntimeOutputUpdate,
};

const PROCESS_BYTES: usize = 8 * 1024 * 1024;

enum Fields<'a> {
    Metadata {
        directory: Option<&'a str>,
        description: Option<&'a str>,
        source: Option<ConversationDescriptionSource>,
        input: Option<(&'a str, bool)>,
    },
    Usage {
        turn: &'a str,
        // Empty turn_id allocates nothing; fill it only after admission.
        counters: ConversationUsage,
    },
}

impl<'a> Fields<'a> {
    fn read(update: &'a Value) -> Option<Self> {
        if !matches!(
            update.get("sessionUpdate")?.as_str()?,
            "session_info_update" | "usage_update"
        ) {
            return None;
        }
        let extension = update.get("_meta")?.get("cma")?;
        if let Some(metadata) = extension.get("runtimeMetadata") {
            let text = |key| metadata.get(key).and_then(Value::as_str);
            return Some(Self::Metadata {
                directory: text("workingDirectory"),
                description: text("description"),
                source: match text("descriptionSource") {
                    Some("provider_title") => Some(ConversationDescriptionSource::ProviderTitle),
                    Some("prompt_preview") => Some(ConversationDescriptionSource::PromptPreview),
                    _ => None,
                },
                input: text("descriptionInput").and_then(|input| {
                    match text("descriptionInputPosition") {
                        Some("current") => Some((input, true)),
                        Some("first") => Some((input, false)),
                        _ => None,
                    }
                }),
            });
        }
        let usage = extension.get("usageSnapshot")?;
        let count = |key| usage.get(key).and_then(Value::as_u64);
        Some(Self::Usage {
            turn: usage.get("turnId")?.as_str()?,
            counters: ConversationUsage {
                turn_id: String::new(),
                input_tokens: count("inputTokens")?,
                output_tokens: count("outputTokens")?,
                last_input_tokens: count("lastInputTokens")?,
                last_output_tokens: count("lastOutputTokens")?,
                context_used: count("contextUsed")?,
                context_window: count("contextWindow"),
                counters_reset: usage
                    .get("countersReset")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        })
    }

    fn weight(&self) -> crate::Result<usize> {
        let strings = match self {
            Self::Metadata {
                directory,
                description,
                input,
                ..
            } => [*directory, *description, input.map(|(input, _)| input)],
            Self::Usage { turn, .. } => [Some(*turn), None, None],
        };
        strings.into_iter().flatten().try_fold(
            RuntimeOutputUpdate::ACCOUNTED_STORAGE_BYTES,
            |bytes, text| bytes.checked_add(text.len()).ok_or_else(capacity),
        )
    }

    fn owned(self) -> ConversationUpdate {
        match self {
            Self::Metadata {
                directory,
                description,
                source,
                input,
            } => ConversationUpdate::Metadata(ConversationMetadata {
                working_directory: directory.map(str::to_owned),
                description: description.map(str::to_owned),
                description_source: source,
                description_input: input.map(|(input, current)| {
                    if current {
                        ConversationDescriptionInput::Current(input.to_owned())
                    } else {
                        ConversationDescriptionInput::First(input.to_owned())
                    }
                }),
            }),
            Self::Usage { turn, mut counters } => {
                counters.turn_id = turn.to_owned();
                ConversationUpdate::Usage(counters)
            }
        }
    }
}

pub(super) fn process_pool() -> Arc<Semaphore> {
    static POOL: OnceLock<Arc<Semaphore>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(Semaphore::new(PROCESS_BYTES)))
        .clone()
}

pub(super) fn admit(
    frame: &Value,
    pool: &Arc<Semaphore>,
    aggregate: Option<&IngressBudget>,
) -> crate::Result<Option<RuntimeOutputUpdate>> {
    if frame.get("method").and_then(Value::as_str) != Some("session/update") {
        return Ok(None);
    }
    let Some(fields) = frame
        .get("params")
        .filter(|params| params.get("sessionId").and_then(Value::as_str).is_some())
        .and_then(|params| params.get("update"))
        .and_then(Fields::read)
    else {
        return Ok(None);
    };
    let storage =
        IngressReservation::try_acquire(pool, aggregate, fields.weight()?).ok_or_else(capacity)?;
    Ok(Some(RuntimeOutputUpdate::accounted(
        fields.owned(),
        Arc::new(storage),
    )))
}

fn capacity() -> AdapterError {
    AdapterError::AtCapacity("runtime observation storage capacity exhausted".into())
}

#[cfg(test)]
#[path = "observation_retention_tests.rs"]
mod tests;
