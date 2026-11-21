use super::{
    catalog::SelectedTool,
    tools::{definitions, ToolSpec},
};
use crate::{auth::AgentCaller, tools::ToolsState};
use cmaeg_core::{
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::{OpenApi, ToSchema};
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PrepareFormArguments {
    event_ref: String,
    message: OutboundMessage,
    idempotency_key: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReadFormPreparationArguments {
    event_ref: String,
}
#[derive(OpenApi)]
#[openapi(components(schemas(PrepareFormArguments, ReadFormPreparationArguments)))]
struct Schemas;
const PREPARE:ToolSpec=ToolSpec{name:"prepareFormReply",schema:"PrepareFormArguments",read_only:false,destructive:false,idempotent:true,open_world:true,
    description:"Prepare one native WhatsApp form without sending a customer message. Requires capabilities.forms and explicit source consent. Keep the same idempotency_key and exact message on every retry. Read preparation status; only Ready permits an explicit replyToEvent with its form_preparation_ref. A pending or uncertain asset is never proof of customer delivery."};
const READ:ToolSpec=ToolSpec{name:"readFormPreparation",schema:"ReadFormPreparationArguments",read_only:true,destructive:false,idempotent:true,open_world:false,
    description:"Read the original event and consumer's durable form preparation or message receipt. Pending/Ready customer_message:not_sent means no send was attempted. Poll after retry_after_ms; once Ready explicitly commit the exact original message with replyToEvent and form_preparation_ref. Never change keys or content after an unknown send."};
pub(super) fn selected(state: &ToolsState) -> Vec<SelectedTool> {
    let Some(forms) = &state.native_whatsapp_forms else {
        return vec![];
    };
    let mut result = vec![SelectedTool {
        spec: &READ,
        definitions: || definitions(Schemas::openapi()),
    }];
    if forms.issuance_enabled {
        result.insert(
            0,
            SelectedTool {
                spec: &PREPARE,
                definitions: || definitions(Schemas::openapi()),
            },
        );
    }
    result
}
pub(super) fn handles(name: &str) -> bool {
    name == PREPARE.name || name == READ.name
}
pub(super) async fn call(
    state: &ToolsState,
    caller: AgentCaller,
    name: &str,
    arguments: Value,
) -> Result<super::results::ToolOutput, Problem> {
    let invalid = || {
        Problem::new(
            ProblemCode::InvalidRequest,
            400,
            "invalid form preparation arguments",
        )
    };
    let receipt = if name == PREPARE.name {
        let args: PrepareFormArguments =
            serde_json::from_value(arguments).map_err(|_| invalid())?;
        state
            .prepare_form_reply(
                caller,
                &crate::http::tools::event_ref(&args.event_ref)?,
                args.message,
                &args.idempotency_key,
            )
            .await?
    } else {
        let args: ReadFormPreparationArguments =
            serde_json::from_value(arguments).map_err(|_| invalid())?;
        state
            .read_form_preparation(caller, &crate::http::tools::event_ref(&args.event_ref)?)
            .await?
    };
    let value = serde_json::to_value(receipt).map_err(|_| invalid())?;
    Ok(super::results::ToolOutput::Envelope(
        super::tools::envelope(value),
    ))
}
