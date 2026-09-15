use super::*;

pub(super) fn error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}})).into_response()
}

pub(super) async fn dispatch(
    backend: &dyn ChannelToolBackend,
    session: Option<&ChannelReplySessionAuthorization>,
    method: &str,
    params: Value,
) -> Result<Value, (i32, &'static str)> {
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .ok_or((-32602, "protocolVersion is required"))?;
            Ok(
                json!({"protocolVersion":if VERSIONS.contains(&version) {version} else {VERSIONS[0]},
                "capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"cma-channel","version":"1"},
                "instructions":"For this channel-created session, use read_channel_context and reply_to_channel to send updates to the client while you work. You can send multiple replies during a turn. Final assistant text is visible only in CMA. Use a new request_key for each intentional message and retain it unchanged for retries. A queued receipt does not mean delivery has completed."}),
            )
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools()),
        "tools/call" => {
            call(
                backend,
                session.ok_or((-32001, "Channel session is not ready"))?,
                params,
            )
            .await
        }
        _ => Err((-32601, "Method not found")),
    }
}

fn tools() -> Value {
    json!({"tools":[{
        "name":"read_channel_context",
        "description":"Read the exact active channel reply context. Call before replying. The returned reference belongs only to this current channel input; never reuse it for a later input. This tool does not send a message.",
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
    },{
        "name":"reply_to_channel",
        "description":"Send a message to the client through the original NyxID channel now, including while you continue working. Requires the current reply_context. Use a unique request_key for each intentional message; retry the same key and text to recover its original receipt. Never change a key to retry an unconfirmed send. Final answers are not automatically sent.",
        "inputSchema":{"type":"object","properties":{
            "reply_context":{"type":"string","pattern":"^brctx_[0-9a-f]{64}$"},
            "request_key":{"type":"string","minLength":1,"maxLength":256},
            "text":{"type":"string","minLength":1,"maxLength":1048576}
        },"required":["reply_context","request_key","text"],"additionalProperties":false},
        "annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":true}
    }]})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
    #[serde(default, rename = "_meta")]
    _meta: Option<Value>,
}
fn empty_object() -> Value {
    json!({})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    reply_context: String,
    request_key: String,
    text: String,
}

async fn call(
    backend: &dyn ChannelToolBackend,
    session: &ChannelReplySessionAuthorization,
    params: Value,
) -> Result<Value, (i32, &'static str)> {
    let call: Call =
        serde_json::from_value(params).map_err(|_| (-32602, "Invalid tool arguments"))?;
    match call.name.as_str() {
        "read_channel_context"
            if call
                .arguments
                .as_object()
                .is_some_and(|args| args.is_empty()) =>
        {
            Ok(match backend.context(session).await {
                Ok(context) => result(json!({"reply_context":context}), false),
                Err(error) => failure(error),
            })
        }
        "reply_to_channel" => {
            let reply: Reply = serde_json::from_value(call.arguments)
                .map_err(|_| (-32602, "Invalid reply arguments"))?;
            if !reply
                .reply_context
                .strip_prefix("brctx_")
                .is_some_and(|suffix| {
                    suffix.len() == 64
                        && suffix
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
                || reply.request_key.is_empty()
                || reply.request_key.len() > 256
                || reply.request_key.chars().any(char::is_control)
                || reply.text.trim().is_empty()
                || reply.text.len() > 1_048_576
            {
                return Err((-32602, "Invalid reply arguments"));
            }
            Ok(
                match backend
                    .reply(
                        session,
                        &reply.reply_context,
                        ChannelReplyToolRequest {
                            request_key: reply.request_key,
                            text: reply.text,
                        },
                    )
                    .await
                {
                    Ok(receipt) => result(
                        serde_json::to_value(receipt).expect("typed receipt serializes"),
                        false,
                    ),
                    Err(error) => failure(error),
                },
            )
        }
        "read_channel_context" => Err((-32602, "Context tool accepts no arguments")),
        _ => Err((-32602, "Unknown tool")),
    }
}

fn result(content: Value, is_error: bool) -> Value {
    json!({"content":[{"type":"text","text":content.to_string()}], "structuredContent":content,"isError":is_error})
}

fn failure(error: OutboxError) -> Value {
    let code = match error {
        OutboxError::RequestConflict => "request_key_conflict",
        OutboxError::Unavailable => "outcome_unconfirmed_retry_same_key",
        OutboxError::SourceMismatch => "reply_context_changed",
        OutboxError::ToolUnavailable => "no_eligible_active_channel_turn",
        OutboxError::RetainedKeyRequired => "retained_reply_key_required",
        OutboxError::UnknownOutcome => "delivery_outcome_unknown",
        OutboxError::Suspended => "delivery_suspended",
        OutboxError::Expired => "authority_expired",
        _ => "reply_unavailable",
    };
    result(json!({"code":code}), true)
}
