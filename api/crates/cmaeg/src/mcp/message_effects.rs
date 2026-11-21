//! New effects are listed after reader rollout; retained references stay usable.
use super::{
    results::ToolOutput,
    tools::{definitions, ToolSpec},
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
    ToolSpec { name: "readMessageCapabilities", description: "Check delete, reaction and optional media-edit support for one exact CMA message reference. Older references and sources without explicit effect authority do not grant effects.", schema: "MessageArguments", read_only: true, destructive: false, idempotent: true, open_world: false },
    ToolSpec { name: "deleteMessage", description: "Delete one confirmed Telegram, Discord, Lark or Feishu bot message, subject to platform age and permission limits. Requires an effect-capable CMA message_ref and stable idempotency_key. Unknown outcomes must not be resent under a new key.", schema: "DeleteArguments", read_only: false, destructive: true, idempotent: true, open_world: true },
    ToolSpec { name: "setMessageReaction", description: "Set or clear the bot's reaction on one confirmed Telegram, Discord, Lark or Feishu bot message, or on the exact received native Telegram, Discord, WhatsApp, Lark or Feishu user message referenced by received_message in event context. Zero or one reaction; paid reactions are unsupported. WhatsApp accepts a Unicode emoji or an empty array to clear, without is_big or custom emoji. Only the bot's own reactions are changed. Lark/Feishu use a platform emoji_type such as THUMBSUP and replace/clear only the reaction CMA added; is_big and custom emoji are unsupported. Requires a stable idempotency_key; never resend an unknown or partially applied outcome under a new key.", schema: "ReactionArguments", read_only: false, destructive: true, idempotent: true, open_world: true },
];
pub(super) fn handles(name: &str) -> bool {
    TOOLS.iter().any(|tool| tool.name == name)
}
pub(super) fn selected(enabled: bool) -> impl Iterator<Item = super::catalog::SelectedTool> {
    TOOLS
        .iter()
        .filter(move |_| enabled)
        .map(|spec| super::catalog::SelectedTool {
            spec,
            definitions: || definitions(Schemas::openapi()),
        })
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
