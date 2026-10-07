//! MCP argument schemas derive from the same message model as the REST routes.
use crate::{auth::AgentCaller, http::tools::event_ref, tools::ToolsState};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventArguments {
    pub event_ref: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyArguments {
    pub event_ref: String,
    pub message: OutboundMessage,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateArguments {
    pub event_ref: String,
    pub message_ref: MessageRef,
    pub message: OutboundMessage,
}
#[derive(OpenApi)]
#[openapi(components(schemas(EventArguments, ReplyArguments, UpdateArguments)))]
struct Schemas;
/// One MCP tool's published description and behaviour hints.
///
/// Each hint is stated per tool rather than derived from a shared flag, so a
/// tool whose hints do not follow the existing pattern needs no special case.
struct ToolSpec {
    name: &'static str,
    description: &'static str,
    /// The argument schema's component name in [`Schemas`].
    schema: &'static str,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
    open_world: bool,
}
const TOOLS: [ToolSpec; 4] = [
    ToolSpec {
        name: "replyToEvent",
        description: "Send a message in reply to an event.",
        schema: "ReplyArguments",
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    },
    ToolSpec {
        name: "updateMessage",
        description: "Update a confirmed message when its transport supports edits.",
        schema: "UpdateArguments",
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: true,
    },
    ToolSpec {
        name: "sendTyping",
        description: "Send a typing indicator when supported.",
        schema: "EventArguments",
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    },
    ToolSpec {
        name: "readEventContext",
        description: "Read the event's public activity and available reply capabilities.",
        schema: "EventArguments",
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    },
];
pub fn list() -> Value {
    let mut document = serde_json::to_value(Schemas::openapi()).unwrap();
    rewrite_refs(&mut document);
    let definitions = document["components"]["schemas"].clone();
    let tools: Vec<_> = TOOLS
        .iter()
        .map(|tool| {
            let mut schema = definitions[tool.schema].clone();
            schema["$defs"] = definitions.clone();
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": schema,
                "annotations": {
                    "readOnlyHint": tool.read_only,
                    "destructiveHint": tool.destructive,
                    "idempotentHint": tool.idempotent,
                    "openWorldHint": tool.open_world,
                },
            })
        })
        .collect();
    json!({"tools":tools})
}
fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "$ref" {
                    if let Some(reference) = value
                        .as_str()
                        .and_then(|value| value.strip_prefix("#/components/schemas/"))
                    {
                        *value = format!("#/$defs/{reference}").into();
                    }
                } else {
                    rewrite_refs(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_refs(value);
            }
        }
        _ => {}
    }
}
pub async fn call(
    state: &ToolsState,
    caller: AgentCaller,
    name: &str,
    arguments: Value,
) -> Result<Value, Problem> {
    let invalid = || Problem::new(ProblemCode::InvalidRequest, 400, "invalid tool arguments");
    let value = match name {
        "replyToEvent" => {
            let args: ReplyArguments = serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .reply(caller, &event_ref(&args.event_ref)?, args.message)
                    .await?,
            )
        }
        "updateMessage" => {
            let args: UpdateArguments = serde_json::from_value(arguments).map_err(|_| invalid())?;
            let (_, event) = state
                .authorize_event(caller, &event_ref(&args.event_ref)?)
                .await?;
            let (_, message) = state.authorize_message(caller, &args.message_ref).await?;
            let same_event = message.channel_id == event.channel_id && message.event_id == event.event_id;
            if !same_event {
                let context=state.context(caller,&event_ref(&args.event_ref)?).await?;
                let permitted = context.activity_verified && message.channel_id==event.channel_id && matches!(&context.activity.kind,cmaeg_core::activity::ActivityKind::Interaction(result) if result.values.get("message_ref").and_then(Value::as_str)==Some(args.message_ref.as_str()));
                if !permitted {return Err(Problem::new(ProblemCode::Forbidden,403,"message does not belong to this event"));}
            }
            serde_json::to_value(
                state
                    .update(caller, &args.message_ref, args.message)
                    .await?,
            )
        }
        "sendTyping" => {
            let args: EventArguments = serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(state.typing(caller, &event_ref(&args.event_ref)?).await?)
        }
        "readEventContext" => {
            let args: EventArguments = serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(state.context(caller, &event_ref(&args.event_ref)?).await?)
        }
        _ => {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "unknown tool",
            ))
        }
    }
    .map_err(|_| Problem::new(ProblemCode::Internal, 500, "tool response unavailable"))?;
    Ok(envelope(value))
}
/// The MCP result for a tool's structured value, mirrored as text content.
///
/// Wrapping happens per call site rather than once for every tool because a
/// later tool may need a content-only result without `structuredContent`.
fn envelope(value: Value) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
}
