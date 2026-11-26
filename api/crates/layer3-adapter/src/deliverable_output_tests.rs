use super::*;
use serde_json::{json, Value};

fn fields(output: Value, status: &str) -> ToolCallUpdateFields {
    serde_json::from_value(json!({
        "status": status,
        "rawInput": {"server":"cma_access","tool":"describe_deliverable","arguments":{"kind":"live_file","path":"fixture.txt"}},
        "rawOutput": output
    }))
    .unwrap()
}

#[test]
fn native_deliverable_refusals_project_the_typed_result_with_local_guidance() {
    for (code, retryable, guidance) in [
        (
            "invalid_resource",
            false,
            "Check the resource kind and exact path or artifact ID.",
        ),
        (
            "not_accessible",
            false,
            "Check the exact resource and the current Agent authority.",
        ),
        (
            "runtime_not_ready",
            true,
            "Wait until this Agent's runtime is ready before retrying.",
        ),
        (
            "too_large",
            false,
            "Produce a smaller file and verify its exact path.",
        ),
        (
            "verification_unavailable",
            true,
            "Wait before retrying the same resource with current authority.",
        ),
        (
            "resource_changed",
            false,
            "Wait until the resource is stable, then verify it again.",
        ),
    ] {
        let summary = format!(
            "This deliverable could not be verified ({code}). Retryable: {retryable}. {guidance}"
        );
        let fields = fields(
            json!({"result":{
                "content":[{"type":"text","text":format!("{summary} Synthetic diagnostic text.")}],
                "structuredContent":{"error":{"code":code,"retryable":retryable}},
                "_meta":null
            },"error":null}),
            "failed",
        );
        assert_eq!(reported_tool_text(&fields), vec![summary.as_str()]);
    }
}

#[test]
fn native_deliverable_transport_and_missing_projection_have_distinct_safe_summaries() {
    let transport = fields(
        json!({"result":null,"error":{"message":"HTTP 503: private upstream diagnostic"}}),
        "failed",
    );
    let summary = reported_tool_text(&transport).join("\n");
    assert!(summary.contains("mcp_transport_error"));
    assert!(summary.contains("Retryable: unknown"));
    assert!(!summary.contains("private upstream diagnostic"));

    for output in [
        Value::Null,
        json!({"result":{"content":[]},"error":null}),
        json!({"result":{"structuredContent":{"error":{"code":"unknown","retryable":true}}},"error":null}),
        json!({"result":{"structuredContent":{"error":{"code":"too_large","retryable":true}}},"error":null}),
    ] {
        let missing = fields(output, "failed");
        let summary = reported_tool_text(&missing).join("\n");
        assert!(summary.contains("result_projection_unavailable"));
        assert!(summary.contains("Retryable: unknown"));
    }
}

#[test]
fn successful_and_unrelated_operations_are_not_projected_as_deliverable_failures() {
    let output = json!({"result":{"content":[{"type":"text","text":"Verified fixture.txt (3 bytes)."}],"structuredContent":{"reference":{"name":"fixture.txt"}}},"error":null});
    assert!(reported_tool_text(&fields(output, "completed")).is_empty());
    let mut other = fields(
        json!({"result":null,"error":{"message":"other tool"}}),
        "failed",
    );
    other.raw_input = Some(json!({"server":"other","tool":"describe_deliverable"}));
    assert!(reported_tool_text(&other).is_empty());
}

#[test]
fn completed_history_call_retains_the_failure_without_a_second_update() {
    let call: ToolCall = serde_json::from_value(json!({
        "toolCallId":"fixture",
        "title":"mcp.cma_access.describe_deliverable",
        "status":"failed",
        "rawInput":{"server":"cma_access","tool":"describe_deliverable"},
        "rawOutput":{"result":null,"error":{"message":"private diagnostic"}}
    }))
    .unwrap();
    assert!(has_reported_initial_output(&call));
    assert!(reported_tool_call_text(&call)
        .join("\n")
        .contains("mcp_transport_error"));
}

#[test]
fn valid_structured_error_survives_missing_or_inconsistent_text() {
    for content in [
        json!([]),
        json!([{"type":"text","text":"This deliverable could not be verified (runtime_not_ready). Retryable: false. Private diagnostic."}]),
    ] {
        let output = fields(
            json!({"result":{
            "content":content,
            "structuredContent":{"error":{"code":"runtime_not_ready","retryable":true}}
        },"error":null}),
            "failed",
        );
        let summary = reported_tool_text(&output).join("\n");
        assert!(summary.contains("(runtime_not_ready). Retryable: true."));
        assert!(summary.contains("Wait until this Agent's runtime is ready"));
        assert!(!summary.contains("Private diagnostic"));
    }
}
