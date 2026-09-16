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
                "instructions":"For this channel-created session, use read_channel_context and reply_to_channel to send updates to the client while you work. You can send multiple replies during a turn. edit_channel_reply intentionally replaces one of your own confirmed Lark/Feishu replies from this active input using its original outbox ID. Final assistant text is visible only in CMA. Use a new request_key for each intentional message and retain it unchanged for retries. A queued receipt does not mean delivery has completed."}),
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
    },{
        "name":"edit_channel_reply",
        "description":"Intentionally replace one confirmed Lark/Feishu reply from this active channel input. Use its opaque outbox ID from the original reply receipt. Other platforms, unsent replies and replies from other inputs are unavailable. Replacement text must fit one 1,900 UTF-16-unit message after preparation. Retain the same request_key, target and text for retries; an unconfirmed edit must not be resubmitted with a new key.",
        "inputSchema":{"type":"object","properties":{
            "reply_context":{"type":"string","pattern":"^brctx_[0-9a-f]{64}$"},
            "original_outbox_id":{"type":"string","pattern":"^bro_[0-9a-f]{64}$"},
            "request_key":{"type":"string","minLength":1,"maxLength":256},
            "text":{"type":"string","minLength":1,"maxLength":1048576}
        },"required":["reply_context","original_outbox_id","request_key","text"],"additionalProperties":false},
        "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":true,"openWorldHint":true}
    },{
        "name":"read_channel_control_context",
        "description":"Read this active channel source's retained control reference when its sender has separate control consent. Operations lists cancellation and steering only when the exact current runtime supports them and control admission is enabled. Availability is advisory and rechecked for each request. This reference grants no authority. This tool does not send a message or control work.",
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false},
        "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    reply_context: String,
    original_outbox_id: String,
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
        "read_channel_control_context"
            if call
                .arguments
                .as_object()
                .is_some_and(|args| args.is_empty()) =>
        {
            Ok(match backend.control_context(session).await {
                Ok(reference) => control_context(reference),
                Err(
                    OutboxError::ToolUnavailable
                    | OutboxError::SourceMismatch
                    | OutboxError::AuthorityUnavailable
                    | OutboxError::Suspended
                    | OutboxError::Expired,
                ) => control_context(ChannelControlContext::default()),
                Err(_) => result(json!({"code":"channel_control_context_unavailable"}), true),
            })
        }
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
            if !valid_reply_arguments(&reply.reply_context, &reply.request_key, &reply.text) {
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
        "edit_channel_reply" => {
            let edit: Edit = serde_json::from_value(call.arguments)
                .map_err(|_| (-32602, "Invalid edit arguments"))?;
            if !valid_reply_arguments(&edit.reply_context, &edit.request_key, &edit.text)
                || !opaque_id(&edit.original_outbox_id, "bro_")
            {
                return Err((-32602, "Invalid edit arguments"));
            }
            Ok(
                match backend
                    .edit(
                        session,
                        &edit.reply_context,
                        ChannelReplyEditRequest {
                            original_outbox_id: edit.original_outbox_id,
                            request_key: edit.request_key,
                            text: edit.text,
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
        "read_channel_context" | "read_channel_control_context" => {
            Err((-32602, "Context tool accepts no arguments"))
        }
        _ => Err((-32602, "Unknown tool")),
    }
}

fn control_context(context: ChannelControlContext) -> Value {
    let availability = if !context.operations.is_empty() {
        "available"
    } else if context.reference.is_some() {
        "execution_unavailable"
    } else {
        "unavailable"
    };
    result(
        json!({"source_control_reference":context.reference,"operations":context.operations,
        "approval_mode":"dashboard_only","availability":availability}),
        false,
    )
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

fn opaque_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 64
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn valid_reply_arguments(context: &str, request_key: &str, text: &str) -> bool {
    opaque_id(context, "brctx_")
        && !request_key.is_empty()
        && request_key.len() <= 256
        && !request_key.chars().any(char::is_control)
        && !text.trim().is_empty()
        && text.len() <= 1_048_576
}
