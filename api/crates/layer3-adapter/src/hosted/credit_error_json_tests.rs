use super::*;

fn legacy(body: &str) -> bool {
    serde_json::Deserializer::from_str(body)
        .into_iter::<serde_json::Value>()
        .next()
        .is_some_and(|result| {
            result.is_ok_and(|value| {
                value.get("error").and_then(serde_json::Value::as_str)
                    == Some("insufficient_credits")
                    && value.get("error_code").and_then(serde_json::Value::as_u64) == Some(11300)
            })
        })
}

#[test]
fn streaming_discriminators_preserve_supported_types_escapes_and_last_keys() {
    for (body, expected) in [
        (
            r#"{"error":"insufficient_credits","error_code":11300}"#,
            true,
        ),
        (
            r#"{"error_code":11300,"err\u006fr":"insufficient_\u0063redits"} transport detail"#,
            true,
        ),
        (
            r#"{"error":"wrong","error":"insufficient_credits","error_code":11300}"#,
            true,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":0,"error_code":11300}"#,
            true,
        ),
        (
            r#"{"error":"insufficient_credits","error":"wrong","error_code":11300}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":11300,"error_code":null}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":11300.0}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":"11300"}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":-11300}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":true}"#,
            false,
        ),
        (
            r#"{"error":["insufficient_credits"],"error_code":11300}"#,
            false,
        ),
        (
            r#"{"error":{"value":"insufficient_credits"},"error_code":11300}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":{"value":11300}}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":18446744073709551616}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":11300,"extra":[null,true,-1,1.5,"s",{"nested":[]}] }"#,
            true,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":11300,"extra":[1,]}"#,
            false,
        ),
        (
            r#"{"error":"insufficient_credits","error_code":11300"#,
            false,
        ),
        (
            r#"[{"error":"insufficient_credits","error_code":11300}]"#,
            false,
        ),
        ("null", false),
        ("not json", false),
    ] {
        assert_eq!(legacy(body), expected, "legacy: {body}");
        assert_eq!(matches(body), expected, "streamed: {body}");
    }
}

#[test]
fn skipped_metadata_still_obeys_parser_recursion_and_input_bounds() {
    for depth in [1, 64, 126, 127, 128, 129, 256] {
        let body = format!(
            r#"{{"error":"insufficient_credits","error_code":11300,"extra":{}0{}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert_eq!(matches(&body), legacy(&body), "depth {depth}");
    }
    let oversized = format!(
        r#"{{"error":"insufficient_credits","error_code":11300,"extra":"{}"}}"#,
        "x".repeat(4 * 1024 * 1024)
    );
    assert!(!matches(&oversized));
}

#[test]
fn both_error_locations_keep_prompt_identity_under_the_default_off_policy() {
    use crate::hosted::acp_turn::{scoped_turn_outcome, turn_outcome_with_policy};
    use crate::{ConversationUpdate, PromptIdentity};
    let diagnostic = r#"unexpected status 402 Payment Required: {"error":"insufficient_credits","error_code":11300,"extra":{"nested":[1,2,3]}}"#;
    for bounded in [false, true] {
        for frame in [
            serde_json::json!({"error":{"message":diagnostic}}),
            serde_json::json!({"result":{"stopReason":"end_turn","_meta":{"jetbrains":{"air":{"sessionFailure":{"title":diagnostic,"severity":"error"}}}}}}),
        ] {
            let prompt = PromptIdentity::new("retained-session", 2, 4);
            let update = scoped_turn_outcome(
                turn_outcome_with_policy(&frame, bounded),
                true,
                Some(prompt.clone()),
            );
            assert!(
                matches!(update, ConversationUpdate::FailedForPrompt {prompt_id, capability, message}
                if prompt_id == prompt && capability.as_deref() == Some("insufficient_credits")
                && !message.contains("nested"))
            );
        }
    }
}
