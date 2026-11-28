//! Translate the managed model gateway's credit refusal at the transport boundary.

use super::acp::Outcome;

const RECOVERY: &str = "The model request failed with HTTP 402 because the model billing account has insufficient credits. Earlier files, tool results, and partial output remain available. Ask the Sandbox owner to top up the account used for model requests, then send a new message to continue. This failed turn will not be replayed automatically.";

pub(super) fn classify(message: &str, bounded_json: bool) -> Option<Outcome> {
    // Match the observed transport diagnostic, then parse its structured body.
    // A number or billing phrase in arbitrary model output is not evidence.
    let body = message
        .trim_start()
        .strip_prefix("unexpected status 402 Payment Required:")?
        .trim_start();
    // The transport can append URL/request diagnostics after the JSON object.
    if bounded_json {
        if !super::credit_error_json::matches(body) {
            return None;
        }
    } else {
        let error = serde_json::Deserializer::from_str(body)
            .into_iter::<serde_json::Value>()
            .next()?
            .ok()?;
        if error.get("error").and_then(serde_json::Value::as_str) != Some("insufficient_credits")
            || error.get("error_code").and_then(serde_json::Value::as_u64) != Some(11300)
        {
            return None;
        }
    }
    Some(Outcome::Failed {
        capability: Some("insufficient_credits".into()),
        message: RECOVERY.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosted::acp_turn::{scoped_turn_outcome, turn_outcome};
    use crate::{ConversationUpdate, PromptIdentity};
    use serde_json::json;

    const REFUSAL: &str = "unexpected status 402 Payment Required: {\"error\":\"insufficient_credits\",\"error_code\":11300,\"message\":\"private upstream detail\"}";

    #[test]
    fn insufficient_credit_errors_are_safe_typed_and_keep_prompt_identity() {
        let prompt = PromptIdentity::new("original-conversation", 1, 3);
        for diagnostic in [REFUSAL.to_owned(), format!("{REFUSAL}\nurl: private-route")] {
            let outcome = turn_outcome(json!({"error": {"message": diagnostic}}));
            assert!(
                matches!(scoped_turn_outcome(outcome, true, Some(prompt.clone())),
                ConversationUpdate::FailedForPrompt { prompt_id, capability, message }
                if prompt_id == prompt && capability.as_deref() == Some("insufficient_credits")
                    && message == RECOVERY && !message.contains("private"))
            );
        }
    }

    #[test]
    fn terminal_metadata_is_classified_but_recovered_warning_stays_successful() {
        for (severity, failed) in [("error", true), ("warning", false)] {
            let outcome = turn_outcome(json!({"result": {
                "stopReason": "end_turn",
                "_meta": {"jetbrains": {"air": {"sessionFailure": {
                    "severity": severity, "title": REFUSAL
                }}}}
            }}));
            if failed {
                assert!(matches!(outcome, Outcome::Failed { capability, message }
                    if capability.as_deref() == Some("insufficient_credits") && message == RECOVERY));
            } else {
                assert!(matches!(outcome, Outcome::Ended(_)));
            }
        }
    }

    #[test]
    fn unrelated_and_ambiguous_diagnostics_do_not_become_credit_failures() {
        for diagnostic in [
            "402 insufficient_credits",
            "unexpected status 402 Payment Required: not json",
            "unexpected status 402 Payment Required: {}",
            "unexpected status 402 Payment Required: {\"error\":\"insufficient_credits\"}",
            "unexpected status 402 Payment Required: {\"error\":\"insufficient_credits\",\"error_code\":11301}",
            "unexpected status 402 Payment Required: {\"error\":\"other_error\",\"error_code\":11300}",
            "unexpected status 403 Forbidden: {\"error\":\"insufficient_credits\",\"error_code\":11300}",
        ] {
            assert!(classify(diagnostic, false).is_none(), "{diagnostic}");
            assert!(matches!(turn_outcome(json!({"error": {
                "message": diagnostic, "data": {"capability": "original_failure"}
            }})), Outcome::Failed { message, capability }
                if message.starts_with("The Agent run failed.") && capability.as_deref() == Some("agent_turn_failed")));
        }
        assert!(matches!(
            turn_outcome(json!({"result": {"stopReason": "end_turn"}})),
            Outcome::Ended(_)
        ));
    }
}
