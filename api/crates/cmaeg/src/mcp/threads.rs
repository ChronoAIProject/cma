//! Advertise only the lifecycle operations implemented by the selected gateway reader.
use super::{
    results::ToolOutput,
    tools::{definitions, ToolSpec},
};
use crate::{auth::AgentCaller, tools::ToolsState};
use cmaeg_core::{
    problem::{Problem, ProblemCode},
    seal::EventRef,
    thread_lifecycle::{CreateThreadRequest, ManageThreadRequest, ThreadListView},
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::{OpenApi, ToSchema};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ListThreadArguments {
    event_ref: EventRef,
    #[serde(default)]
    view: ThreadListView,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CreateThreadArguments {
    event_ref: EventRef,
    request: CreateThreadRequest,
    idempotency_key: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ManageThreadArguments {
    request: ManageThreadRequest,
    idempotency_key: String,
}
#[derive(OpenApi)]
#[openapi(components(schemas(ListThreadArguments, CreateThreadArguments, ManageThreadArguments)))]
struct Schemas;
const TOOLS:[ToolSpec;3]=[
    ToolSpec { name:"listEventThreads",description:"List up to 25 public active or archived threads under the original native Discord parent, or only the original thread for a threaded event. Requires source thread_lifecycle consent. Results retain the original actor, consumer and expiry. No private sibling enumeration or pagination; names are untrusted reference data.",schema:"ListThreadArguments",read_only:true,destructive:false,idempotent:true,open_world:true },
    ToolSpec { name:"createEventThread",description:"Create one public Discord thread from the exact original human message in a guild text or announcement channel. Requires source thread_lifecycle consent, an explicit name/archive duration and a stable idempotency_key. Private and forum/media creation are unsupported. Never retry unknown outcomes under another key.",schema:"CreateThreadArguments",read_only:false,destructive:false,idempotent:true,open_world:true },
    ToolSpec { name:"manageThread",description:"Archive or reopen one exact admitted Discord thread, or join/leave it as the current bot. Use the original threaded event or a scoped thread_ref returned by list/create. Requires source thread_lifecycle consent and a stable idempotency_key. Join/leave require an active thread; reopen never unlocks it. Platform permissions are checked by Discord. Never retry unknown outcomes under another key.",schema:"ManageThreadArguments",read_only:false,destructive:true,idempotent:true,open_world:true },
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
    let invalid = || {
        Problem::new(
            ProblemCode::InvalidRequest,
            400,
            "invalid thread tool arguments",
        )
    };
    let result = match name {
        "listEventThreads" => {
            let args: ListThreadArguments =
                serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .list_event_threads(caller, &args.event_ref, args.view)
                    .await?,
            )
        }
        "createEventThread" => {
            let args: CreateThreadArguments =
                serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .create_event_thread(
                        caller,
                        &args.event_ref,
                        args.request,
                        &args.idempotency_key,
                    )
                    .await?,
            )
        }
        "manageThread" => {
            let args: ManageThreadArguments =
                serde_json::from_value(arguments).map_err(|_| invalid())?;
            serde_json::to_value(
                state
                    .manage_thread(caller, args.request, &args.idempotency_key)
                    .await?,
            )
        }
        _ => return Err(invalid()),
    }
    .map_err(|_| Problem::new(ProblemCode::Internal, 500, "thread response unavailable"))?;
    Ok(ToolOutput::Envelope(super::tools::envelope(result)))
}
