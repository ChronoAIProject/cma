//! Turning what the provider says about a stopped sandbox into a verdict.
//!
//! The one place a provider's word for a death is read. Above this, the death
//! has a name from this layer's own closed set and the provider's spelling is
//! gone -- CLAUDE.md section 3, and the same crossing `hosted/mod.rs` already
//! makes for resource quantities.

use crate::runtime::RuntimeLossReason;
use crate::sandbox::Sandbox;

/// Why this sandbox stopped, where it has stopped and said.
///
/// `None` while it is still serving, and `None` when it named nothing.
pub(crate) fn loss_reason(sandbox: &Sandbox) -> Option<RuntimeLossReason> {
    if sandbox.is_running() {
        return None;
    }
    let word = sandbox.status.as_ref()?.reason.as_deref()?.trim();
    if word.is_empty() {
        return None;
    }
    // Case-insensitively, because the provider is not consistent about it:
    // its own `state` reads `Running` while its `reason` reads `RUNNING`.
    let reason = match word.to_ascii_lowercase().as_str() {
        // This arm has not been observed against this provider. The one real
        // OOM recorded in #464 returned `Error`, which maps to `Failed`.
        "oomkilled" | "outofmemory" => RuntimeLossReason::OutOfMemory,
        "completed" | "succeeded" => RuntimeLossReason::Completed,
        "error" | "failed" => RuntimeLossReason::Failed,
        "evicted" | "nodelost" | "shutdown" => RuntimeLossReason::Displaced,
        _ => {
            // Log once per listing at debug level. The word stops here and can
            // extend this table later from evidence rather than guesswork.
            tracing::debug!(
                sandbox = %sandbox.id,
                provider_reason = %word,
                "the provider named a stop reason this build does not recognise"
            );
            RuntimeLossReason::Unrecognised
        }
    };
    Some(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(state: Option<&str>, reason: Option<&str>) -> Sandbox {
        let status = state.map(|state| {
            serde_json::json!({
                "state": state,
                "reason": reason,
            })
        });
        serde_json::from_value(serde_json::json!({
            "id": "sandbox-a",
            "status": status,
        }))
        .expect("the sandbox fixture is valid")
    }

    #[test]
    fn a_serving_sandbox_has_no_loss_reason() {
        let value: Sandbox =
            serde_json::from_str(r#"{"id":"a","status":{"state":"Running","reason":"RUNNING"}}"#)
                .expect("the provider fixture is valid");
        assert_eq!(loss_reason(&value), None);
    }

    #[test]
    fn a_stopped_sandbox_with_no_reason_yields_no_verdict() {
        let no_reason: Sandbox = serde_json::from_str(r#"{"id":"a","status":{"state":"Failed"}}"#)
            .expect("the provider fixture is valid");
        let no_status: Sandbox =
            serde_json::from_str(r#"{"id":"a"}"#).expect("the provider fixture is valid");

        assert_eq!(loss_reason(&no_reason), None);
        assert_eq!(loss_reason(&no_status), None);
        assert_eq!(loss_reason(&sandbox(Some("Failed"), Some("   "))), None);
    }

    #[test]
    fn the_provider_words_this_build_knows_each_map_to_one_verdict() {
        let cases = [
            ("oomkilled", RuntimeLossReason::OutOfMemory),
            ("outofmemory", RuntimeLossReason::OutOfMemory),
            ("completed", RuntimeLossReason::Completed),
            ("succeeded", RuntimeLossReason::Completed),
            ("error", RuntimeLossReason::Failed),
            ("failed", RuntimeLossReason::Failed),
            ("evicted", RuntimeLossReason::Displaced),
            ("nodelost", RuntimeLossReason::Displaced),
            ("shutdown", RuntimeLossReason::Displaced),
        ];
        for (word, expected) in cases {
            assert_eq!(
                loss_reason(&sandbox(Some("Failed"), Some(word))),
                Some(expected),
                "provider word {word}"
            );
        }
    }

    #[test]
    fn casing_does_not_change_the_verdict() {
        for word in ["OOMKilled", "oomkilled", "OOMKILLED"] {
            assert_eq!(
                loss_reason(&sandbox(Some("Failed"), Some(word))),
                Some(RuntimeLossReason::OutOfMemory)
            );
        }
    }

    #[test]
    fn a_word_this_build_does_not_know_is_unrecognised_not_absent() {
        assert_eq!(
            loss_reason(&sandbox(Some("Failed"), Some("SomethingNew"))),
            Some(RuntimeLossReason::Unrecognised)
        );
    }

    #[test]
    fn no_provider_word_reaches_the_verdict() {
        let rendered = format!(
            "{:?}",
            loss_reason(&sandbox(Some("Failed"), Some("SomethingNew")))
        );
        assert_eq!(rendered, "Some(Unrecognised)");
        assert!(!rendered.contains("SomethingNew"));
    }
}
