use crate::ToolCallStatus;
use serde_json::Value;
use std::borrow::Cow;

const PROJECTION_UNAVAILABLE: &str = "Deliverable result unavailable (result_projection_unavailable). Retryable: unknown. Check the operation history and runtime readiness before verifying the same resource again.";
const TRANSPORT_ERROR: &str = "MCP call failed before a typed deliverable reply (mcp_transport_error). Retryable: unknown. Check the runtime and MCP connection before retrying the same resource.";

pub(crate) fn is_failed_deliverable(
    status: Option<&ToolCallStatus>,
    input: Option<&Value>,
) -> bool {
    status == Some(&ToolCallStatus::Failed)
        && input.is_some_and(|input| {
            input.get("server").and_then(Value::as_str) == Some("cma_access")
                && input.get("tool").and_then(Value::as_str) == Some("describe_deliverable")
        })
}

pub(crate) fn failure_summary<'a>(
    status: Option<&ToolCallStatus>,
    input: Option<&Value>,
    output: Option<&'a Value>,
) -> Option<Cow<'a, str>> {
    if !is_failed_deliverable(status, input) {
        return None;
    }
    let Some(output) = output else {
        return Some(Cow::Borrowed(PROJECTION_UNAVAILABLE));
    };
    if output.get("error").is_some_and(|error| !error.is_null()) {
        // Native transport diagnostics can contain credentials, URLs or response bodies.
        return Some(Cow::Borrowed(TRANSPORT_ERROR));
    }
    Some(business_failure(output).unwrap_or(Cow::Borrowed(PROJECTION_UNAVAILABLE)))
}

fn business_failure(output: &Value) -> Option<Cow<'_, str>> {
    let result = output.get("result")?;
    let error = result.pointer("/structuredContent/error")?;
    let code = error.get("code")?.as_str()?;
    if !matches!(
        code,
        "invalid_resource"
            | "not_accessible"
            | "verification_unavailable"
            | "verification_failed"
            | "too_large"
            | "resource_changed"
            | "runtime_not_ready"
            | "runtime_unavailable"
            | "runtime_busy"
            | "runtime_unsupported"
    ) {
        return None;
    }
    let retryable = error.get("retryable")?.as_bool()?;
    let prefix =
        format!("This deliverable could not be verified ({code}). Retryable: {retryable}.");
    // The CMA tool supplies the public, actionable text alongside the typed error.
    if result.pointer("/content/0/type").and_then(Value::as_str) == Some("text") {
        if let Some(text) = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .filter(|text| text.starts_with(&prefix))
        {
            return Some(Cow::Borrowed(text));
        }
    }
    let guidance = if retryable {
        "Wait before retrying the same resource with current authority."
    } else {
        "Check the resource, runtime and current authority before trying again."
    };
    Some(Cow::Owned(format!("{prefix} {guidance}")))
}
