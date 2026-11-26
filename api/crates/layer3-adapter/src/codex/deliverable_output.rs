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

pub(crate) fn failure_summary(
    status: Option<&ToolCallStatus>,
    input: Option<&Value>,
    output: Option<&Value>,
) -> Option<Cow<'static, str>> {
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
    Some(business_failure(output).map_or(Cow::Borrowed(PROJECTION_UNAVAILABLE), Cow::Owned))
}

fn business_failure(output: &Value) -> Option<String> {
    let error = output.pointer("/result/structuredContent/error")?;
    let code = error.get("code")?.as_str()?;
    let retryable = error.get("retryable")?.as_bool()?;
    // The bridge supplies the result envelope. Only retain known error codes
    // and local guidance so arbitrary result text cannot enter the Agent log.
    let guidance = match (code, retryable) {
        ("invalid_resource", false) => "Check the resource kind and exact path or artifact ID.",
        ("not_accessible", false) => "Check the exact resource and the current Agent authority.",
        ("too_large", false) => "Produce a smaller file and verify its exact path.",
        ("resource_changed", false) => "Wait until the resource is stable, then verify it again.",
        ("runtime_not_ready", true) => "Wait until this Agent's runtime is ready before retrying.",
        ("runtime_unavailable" | "runtime_busy" | "verification_unavailable", true) => {
            "Wait before retrying the same resource with current authority."
        }
        ("runtime_unsupported", false) => {
            "Use a runtime that supports the required file operation."
        }
        ("verification_unavailable", false) => {
            "Check service availability and configuration before trying again."
        }
        ("verification_failed", false) => "Use the error code when contacting support.",
        _ => return None,
    };
    Some(format!(
        "This deliverable could not be verified ({code}). Retryable: {retryable}. {guidance}"
    ))
}
