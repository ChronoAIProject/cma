use layer2_core::sandbox::agent_access::{AgentAccessRequest, AgentAccessStatus};
use serde_json::{json, Value};

pub(super) fn admitted(request: AgentAccessRequest) -> Value {
    let failed = request.status == AgentAccessStatus::Failed;
    let message = match request.status {
        AgentAccessStatus::Pending => "The request awaits owner approval. Reusing its key only observes this attempt; wait for CMA confirmation before using the service.",
        AgentAccessStatus::Connecting | AgentAccessStatus::Applying => "The approved request is still being applied. Reuse this request key to observe it; do not create another attempt or use the service before CMA confirms success.",
        AgentAccessStatus::Succeeded => "CMA confirmed this access update. Continue the original task in this conversation without creating another access request.",
        AgentAccessStatus::Denied => "The owner denied this request. Its outcome is permanent for this key. Do not automatically resubmit or infer approval.",
        AgentAccessStatus::Failed => "This attempt failed; reusing its request_key returns the same failure and does not restart it. If the owner asks to try again, submit next_request as a new pending attempt. If your tool schema supports retry_of, also pass next_request_retry_of as retry_of; otherwise the three-field next_request works with the original cached schema, without restarting the conversation. The failed record remains unchanged and fresh owner approval is required. Do not retry the service until CMA confirms the new access update.",
    };
    let mut value = json!({
        "request_id":request.id,"agent_id":request.agent_id,"status":request.status,
        "services":request.services,"message":message,
    });
    if let Some(error) = &request.error {
        value["error"] = error.clone().into();
    }
    if let Some(retry_of) = &request.retry_of {
        value["retry_of"] = retry_of.clone().into();
    }
    if failed {
        // The deterministic successor key lets uncertain retries converge on
        // one new attempt without reopening or reapproving the failed record.
        value["next_request"] = json!({
            "request_key":format!("retry:{}", request.id),
            "services":request.services,"reason":request.reason,
        });
        value["next_request_retry_of"] = request.id.clone().into();
        value["requires_owner_approval"] = true.into();
    }
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,
        "isError":failed || request.status == AgentAccessStatus::Denied})
}
