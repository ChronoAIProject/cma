use super::*;
use layer2_core::sandbox::agent_access_tool::AgentAccessProposal;

const TOOL: &str = "agent_key_scope_revision";

pub(super) async fn invoke(state: &AccessState, capability: &str, rpc: Rpc) -> Response {
    if rpc.method == "tools/call" {
        let requester = match state.auth.authenticate(capability).await {
            Ok(requester) => requester,
            Err(error) => return auth_error(error),
        };
        if !state.rate.admit(capability) {
            return busy();
        }
        let Some(id) = rpc.id else {
            return rpc_error(Value::Null, -32600, "Tool calls require a request ID");
        };
        let call: Call = match serde_json::from_value(rpc.params) {
            Ok(value) => value,
            Err(_) => return rpc_error(id, -32602, "Invalid tool arguments"),
        };
        if call.name != TOOL {
            return rpc_error(id, -32602, "Unknown tool");
        }
        let proposal: AgentAccessProposal = match serde_json::from_value(call.arguments) {
            Ok(value) => value,
            Err(_) => return rpc_error(id, -32602, "Invalid tool arguments"),
        };
        if proposal.validate().is_err() {
            return rpc_error(id, -32602, "Invalid tool arguments");
        }
        return match state.admission.request(requester, proposal).await {
            Ok(request) => {
                let value = json!({
                    "request_id":request.id,"agent_id":request.agent_id,"status":request.status,
                    "services":request.services,
                    "message":"The request is recorded. Service access changes only after explicit owner approval and CMA confirmation.",
                });
                result(
                    id,
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}),
                )
            }
            Err(error) => {
                let message = match error {
                    AgentAccessError::Conflict => {
                        "This request key is already bound to a different proposal"
                    }
                    AgentAccessError::Invalid => "The access request is invalid",
                    AgentAccessError::NotFound | AgentAccessError::Forbidden => {
                        "This Agent can no longer request access"
                    }
                    AgentAccessError::Unavailable => {
                        "Access request could not be confirmed; retry with the same request_key"
                    }
                };
                result(
                    id,
                    json!({"content":[{"type":"text","text":message}],"isError":true}),
                )
            }
        };
    }
    if let Err(error) = state.auth.authenticate_setup(capability).await {
        return auth_error(error);
    }
    if !state.rate.admit(capability) {
        return busy();
    }
    if rpc.method == "notifications/initialized" {
        return if rpc.id.is_none() && (rpc.params.is_null() || rpc.params.is_object()) {
            StatusCode::ACCEPTED.into_response()
        } else {
            rpc_error(
                rpc.id.unwrap_or(Value::Null),
                -32600,
                "Invalid notification",
            )
        };
    }
    let Some(id) = rpc.id else {
        return StatusCode::ACCEPTED.into_response();
    };
    match rpc.method.as_str() {
        "initialize" => {
            let valid = rpc
                .params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty() && value.len() <= 32)
                && rpc.params.get("capabilities").is_some_and(Value::is_object)
                && rpc
                    .params
                    .get("clientInfo")
                    .and_then(|value| value.get("name"))
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty() && value.len() <= 128);
            if !valid {
                return rpc_error(id, -32602, "Invalid initialize parameters");
            }
            result(
                id,
                json!({
                    "protocolVersion":VERSION,
                    "serverInfo":{"name":"cma-agent-access","version":"1"},
                    "capabilities":{"tools":{"listChanged":false}},
                    "instructions":"Use agent_key_scope_revision when a required NyxID service lacks permission. Reuse request_key on retries. The tool only requests owner consent; wait for CMA to confirm the update before retrying the service.",
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => {
            if rpc.params.get("cursor").is_some() {
                return rpc_error(id, -32602, "This tool list has no continuation cursor");
            }
            result(id, list())
        }
        _ => rpc_error(id, -32601, "Method not found"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    arguments: Value,
    #[serde(default, rename = "_meta")]
    _meta: Option<Value>,
}

fn list() -> Value {
    json!({"tools":[{
        "name":TOOL,
        "title":"Request owner approval for service access",
        "description":"Ask the Agent owner to connect and authorize required NyxID services. This does not grant access. Keep the same request_key when retrying the same request. CMA notifies the original Agent after the authorized key update is confirmed.",
        "inputSchema":{
            "type":"object","additionalProperties":false,
            "required":["request_key","services","reason"],
            "properties":{
                "request_key":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_.:-]+$","description":"Stable idempotency key for this exact proposal; reuse it on retries."},
                "services":{"type":"array","minItems":1,"maxItems":32,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_-]+$"}},
                "reason":{"type":"string","minLength":1,"maxLength":2048,"description":"Explain to the owner why the Agent needs these services."},
            },
        },
        "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false},
    }]})
}
