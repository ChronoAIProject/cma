use super::*;
use layer2_core::sandbox::answer_deliverables::DeliverableResource;

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
        Err(_) => result(
            id,
            json!({"content":[{"type":"text","text":"This deliverable could not be verified. Check the exact resource, its current availability and authority. Do not invent a reference or claim publication. Document references come from publish_document or update_document receipts."}],"isError":true}),
        ),
    }
}

pub(super) fn tool() -> Value {
    json!({"name":"describe_deliverable","title":"Verify an answer deliverable",
        "description":"After actually producing a file, verify its exact home-relative path, or select an already published Agent artifact by its exact ID. Copy the returned marker unchanged on a standalone line in the final answer delivering that result. This reads and hashes bounded bytes; it never publishes private files, takes a snapshot, scans a directory, infers a name, or grants sharing. Do not use absolute paths or credentials. A changed live file cannot later be downloaded as its old result. Document markers come from publish_document/update_document receipts.",
        "inputSchema":{"type":"object","additionalProperties":false,"required":["kind"],"properties":{
            "kind":{"type":"string","enum":["live_file","agent_artifact"]},
            "path":{"type":"string","minLength":1,"maxLength":1024},
            "id":{"type":"string","pattern":"^art_[a-f0-9]{64}$"}
        },"oneOf":[{"required":["kind","path"],"properties":{"kind":{"const":"live_file"}}},{"required":["kind","id"],"properties":{"kind":{"const":"agent_artifact"}}}]},
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}})
}
