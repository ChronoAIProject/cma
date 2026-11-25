use super::*;
use layer2_core::{
    document_artifacts::DocumentError,
    resource_access::{MembershipError, ResourceAccessError, ResourceAccessStoreError},
    sandbox::{
        agent_filesystem::AgentFileError,
        answer_deliverables::{DeliverableError, DeliverableResource},
        AgentReadError,
    },
};

pub(super) async fn invoke(
    state: &AccessState,
    capability: &str,
    id: Value,
    arguments: Value,
) -> Response {
    let Some(service) = &state.deliverables else {
        return rpc_error(id, -32602, "Unknown tool");
    };
    let resource: DeliverableResource = match serde_json::from_value(arguments) {
        Ok(resource) => resource,
        Err(_) => return rpc_error(id, -32602, "Invalid deliverable arguments"),
    };
    match service
        .describe_for_agent(state.auth.as_ref(), capability, resource)
        .await
    {
        Ok(deliverable) => result(
            id,
            json!({"content":[{"type":"text","text":format!(
            "Verified {} ({} bytes). Include this unchanged marker on a standalone line in the final answer that delivers it:\n{}\nLive files remain live and private; this does not publish a snapshot.",
            deliverable.reference.name, deliverable.reference.size_bytes, deliverable.marker,
        )}], "structuredContent":deliverable,"isError":false}),
        ),
        Err(error) => result(id, failure(error)),
    }
}

pub(super) fn failure(error: DeliverableError) -> Value {
    const NOT_ACCESSIBLE: (&str, bool, &str) = (
        "not_accessible",
        false,
        "The resource is not accessible to this Agent. Check the exact resource and the current Agent authority; do not probe other resources.",
    );
    const UNAVAILABLE: (&str, bool, &str) = (
        "verification_unavailable",
        true,
        "A verification dependency is temporarily unavailable. Wait before retrying the same resource with current authority.",
    );
    const FAILED: (&str, bool, &str) = (
        "verification_failed",
        false,
        "The resource could not be safely verified. Seek support with this reason code; do not include private paths, content or credentials.",
    );
    let (code, retryable, instruction) = match error {
        DeliverableError::Invalid => (
            "invalid_resource", false,
            "Check the resource kind, exact file path (relative to your working directory or the Sandbox home) or published Agent artifact ID. The request or its verification data is invalid. Document references come from publish_document or update_document receipts.",
        ),
        // Invalid file requests also include runtime permission refusals.
        DeliverableError::NotFound
        | DeliverableError::File(AgentFileError::NotFound | AgentFileError::Invalid | AgentFileError::NameTooLong)
        | DeliverableError::Answer(AgentReadError::NotFound | AgentReadError::Forbidden)
        | DeliverableError::Document(DocumentError::NotFound) => NOT_ACCESSIBLE,
        DeliverableError::File(AgentFileError::TooLarge) => (
            "too_large", false,
            "The file exceeds the 32 MiB verification limit. Produce a smaller file and verify its exact path.",
        ),
        DeliverableError::Changed | DeliverableError::File(AgentFileError::Conflict) => (
            "resource_changed", false,
            "The file bytes or runtime identity changed during verification. Once stable, verify the intended file again and use only the newly returned marker.",
        ),
        DeliverableError::File(AgentFileError::NotReady) => (
            "runtime_not_ready", true,
            "Wait until this Agent's runtime is ready before retrying the same resource.",
        ),
        DeliverableError::File(AgentFileError::Unavailable) => (
            "runtime_unavailable", true,
            "The runtime file operation is temporarily unavailable. Wait before retrying the same resource.",
        ),
        DeliverableError::File(AgentFileError::RuntimeCapacity(_))
        | DeliverableError::Answer(AgentReadError::RuntimeCapacity(_)) => (
            "runtime_busy", true,
            "The runtime has no available request capacity. Wait before retrying the same resource.",
        ),
        DeliverableError::File(AgentFileError::Unsupported) => (
            "runtime_unsupported", false,
            "This runtime does not support the required file operation. Repeating the request cannot add that capability.",
        ),
        DeliverableError::Artifact(access)
        | DeliverableError::File(AgentFileError::Access(access))
        | DeliverableError::Answer(AgentReadError::Access(access)) => match access {
            ResourceAccessError::Store(ResourceAccessStoreError::Unavailable)
            | ResourceAccessError::Membership(MembershipError::Unavailable) => UNAVAILABLE,
            _ => NOT_ACCESSIBLE,
        },
        DeliverableError::Answer(AgentReadError::Unavailable(_))
        | DeliverableError::Document(DocumentError::Unavailable) => UNAVAILABLE,
        DeliverableError::Unavailable => (
            "verification_unavailable", false,
            "The required verification capability is unavailable. Check service availability and configuration before trying again.",
        ),
        DeliverableError::Answer(_) | DeliverableError::Document(_) => FAILED,
    };
    json!({
        "content":[{"type":"text","text":format!(
            "This deliverable could not be verified ({code}). Retryable: {retryable}. {instruction} Do not invent a reference or claim publication."
        )}],
        "structuredContent":{"error":{"code":code,"retryable":retryable}},
        "isError":true
    })
}

pub(super) fn tool() -> Value {
    json!({"name":"describe_deliverable","title":"Verify an answer deliverable",
        "description":"After actually producing a file, verify its exact path: relative to your working directory or to the Sandbox home, or absolute under /home/cma. Or select an already published Agent artifact by its exact ID. Copy the returned marker unchanged on a standalone line in the final answer delivering that result. This reads and hashes bounded bytes; it never publishes private files, takes a snapshot, scans a directory, infers a name, or grants sharing. Do not use paths outside /home/cma or credentials. A changed live file cannot later be downloaded as its old result. Document markers come from publish_document/update_document receipts. Failures return a safe error.code and error.retryable; retryable means a delayed retry may succeed, not that the resource was verified. Follow the error's guidance and never retry indefinitely.",
        "inputSchema":{"type":"object","additionalProperties":false,"required":["kind"],"properties":{
            "kind":{"type":"string","enum":["live_file","agent_artifact"]},
            "path":{"type":"string","minLength":1,"maxLength":1024},
            "id":{"type":"string","pattern":"^art_[a-f0-9]{64}$"}
        },"oneOf":[{"required":["kind","path"],"properties":{"kind":{"const":"live_file"}}},{"required":["kind","id"],"properties":{"kind":{"const":"agent_artifact"}}}]},
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})
}
