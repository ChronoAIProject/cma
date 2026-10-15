use crate::{ConversationMetadata, ConversationUpdate, ConversationUsage};
use serde_json::Value;

/// Extension owned by the hosting bridge, decoded below the domain boundary.
/// Unrelated ACP metadata remains opaque; it cannot alter domain settings.
pub(super) fn observation(value: &Value) -> Option<ConversationUpdate> {
    if !matches!(
        value.get("sessionUpdate")?.as_str()?,
        "session_info_update" | "usage_update"
    ) {
        return None;
    }
    let extension = value.pointer("/_meta/cma")?;
    if let Some(metadata) = extension.get("runtimeMetadata") {
        let text = |key: &str| metadata.get(key).and_then(Value::as_str).map(str::to_owned);
        return Some(ConversationUpdate::Metadata(ConversationMetadata {
            working_directory: text("workingDirectory"),
            description: text("description"),
        }));
    }
    let usage = extension.get("usageSnapshot")?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64);
    Some(ConversationUpdate::Usage(ConversationUsage {
        turn_id: usage.get("turnId")?.as_str()?.to_owned(),
        input_tokens: count("inputTokens")?,
        output_tokens: count("outputTokens")?,
        last_input_tokens: count("lastInputTokens")?,
        last_output_tokens: count("lastOutputTokens")?,
        context_used: count("contextUsed")?,
        context_window: usage.get("contextWindow").and_then(Value::as_u64),
        counters_reset: usage
            .get("countersReset")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_decoding_never_guesses_usage_and_ignores_unrelated_metadata() {
        let value = serde_json::json!({"sessionUpdate":"usage_update","used":50,"size":100,"_meta":{"cma":{"usageSnapshot":{"turnId":"t","inputTokens":30,"outputTokens":5,"lastInputTokens":20,"lastOutputTokens":2,"contextUsed":22,"contextWindow":100}}}});
        assert!(matches!(
            observation(&value),
            Some(ConversationUpdate::Usage(ConversationUsage {
                input_tokens: 30,
                output_tokens: 5,
                ..
            }))
        ));
        let mut invalid = value.clone();
        invalid["_meta"]["cma"]["usageSnapshot"]["inputTokens"] = serde_json::json!(-1);
        assert!(observation(&invalid).is_none());
        assert!(observation(
            &serde_json::json!({"sessionUpdate":"usage_update","used":100,"size":200})
        )
        .is_none());
        assert!(observation(&serde_json::json!({"sessionUpdate":"session_info_update","_meta":{"other":{"cwd":"/different"}}})).is_none());
    }
}
