//! New effects are listed after reader rollout; retained references stay usable.
use super::{
    results::ToolOutput,
    tools::{definitions, describe, ToolSpec},
};
use crate::{auth::AgentCaller, tools::ToolsState};
use cmaeg_core::{
    ids::MessageRef,
    message_effect::MessageReactionRequest,
    problem::{Problem, ProblemCode},
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::{OpenApi, ToSchema};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct MessageArguments {
    message_ref: MessageRef,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct DeleteArguments {
    message_ref: MessageRef,
    idempotency_key: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReactionArguments {
    message_ref: MessageRef,
    idempotency_key: String,
    request: MessageReactionRequest,
}
#[derive(OpenApi)]
#[openapi(components(schemas(MessageArguments, DeleteArguments, ReactionArguments)))]
struct Schemas;
const TOOLS: [ToolSpec; 3] = [
    ToolSpec { name: "readMessageCapabilities", description: "Check delete and reaction support for one confirmed CMA message reference. Relay transports and older references do not grant effects.", schema: "MessageArguments", read_only: true, destructive: false, idempotent: true, open_world: false },
    ToolSpec { name: "deleteMessage", description: "Delete one confirmed native Telegram bot message, subject to Telegram age and permission limits. Requires a CMA-issued message_ref and stable idempotency_key. Unknown outcomes must not be resent under a new key.", schema: "DeleteArguments", read_only: false, destructive: true, idempotent: true, open_world: true },
    ToolSpec { name: "setMessageReaction", description: "Set or clear the native Telegram bot's reaction on one confirmed CMA bot message. Zero or one reaction; paid reactions are unsupported. Requires a stable idempotency_key; never resend an unknown outcome under a new key.", schema: "ReactionArguments", read_only: false, destructive: true, idempotent: true, open_world: true },
];
pub(super) fn handles(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}
pub(super) fn list(media: bool, effects: bool) -> Value {
    let mut value = super::tools::list(media);
    if effects {
        let definitions = definitions(Schemas::openapi());
        value["tools"]
            .as_array_mut()
            .expect("tool array")
            .extend(TOOLS.iter().map(|tool| describe(tool, &definitions)));
    }
    value
}
pub(super) async fn call(
    state: &ToolsState,
    caller: AgentCaller,
    name: &str,
    arguments: Value,
) -> Result<ToolOutput, Problem> {
    let invalid = || Problem::new(ProblemCode::InvalidRequest, 400, "invalid tool arguments");
    let value = match name {
        "readMessageCapabilities" => {
            let args: MessageArguments =
                serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .message_effect_capabilities(caller, &args.message_ref)
                    .await?,
            )
        }
        "deleteMessage" => {
            let args: DeleteArguments = serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .delete_message(caller, &args.message_ref, &args.idempotency_key)
                    .await?,
            )
        }
        "setMessageReaction" => {
            let args: ReactionArguments =
                serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .set_message_reaction(
                        caller,
                        &args.message_ref,
                        &args.idempotency_key,
                        args.request,
                    )
                    .await?,
            )
        }
        _ => return Err(invalid()),
    }
    .map_err(|_| Problem::new(ProblemCode::Internal, 500, "tool response unavailable"))?;
    Ok(ToolOutput::Envelope(super::tools::envelope(value)))
}
