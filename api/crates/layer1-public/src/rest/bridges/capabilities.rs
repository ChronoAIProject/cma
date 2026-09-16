//! Additive owner/admin capability diagnostics and their OpenAPI projection.
#![allow(dead_code)]
use super::*;

#[utoipa::path(get,path="/api/v1/bridge-connectors/capabilities",tag="bridges",params(("organization"=Option<String>,Query)),responses((status=200,body=BridgeCapabilityCatalogShape),(status=400,body=super::super::ProblemDocument),(status=401,body=super::super::ProblemDocument),(status=403,body=super::super::ProblemDocument),(status=404,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn capabilities(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    params: Result<Query<OwnerParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let owner = owner(&caller, params.organization);
    respond(
        service
            .capabilities(&caller.context, credential(&caller), &owner)
            .await,
        StatusCode::OK,
    )
}

use utoipa::ToSchema;
#[derive(ToSchema)]
pub(crate) struct BridgeCapabilityCatalogShape {
    version: u16,
    source_revision: Option<String>,
    connection_validation_available: bool,
    connectors: Vec<BridgeConnectorCapabilitiesShape>,
    placements: Vec<BridgePlacementCapabilitiesShape>,
}
#[derive(ToSchema)]
struct BridgePlacementCapabilitiesShape {
    kind: BridgeSharedPlacementKindShape,
    availability: BridgePlacementAvailabilityShape,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgeSharedPlacementKindShape {
    WorkspacePerPartition,
    WorktreeAgentPerPartition,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgePlacementAvailabilityShape {
    Available,
    RuntimeUnavailable,
    ContinuityUnverified,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgeCapabilityEvidenceShape {
    ContractTested,
    UpstreamOnly,
    IngestOnly,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgeAttachmentInputShape {
    DownloadRequired,
    Unavailable,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgeOutputFormatShape {
    EscapedMarkdown,
    PlainText,
    Unavailable,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
enum BridgeStableThreadShape {
    TelegramForumTopic,
    DiscordChannelThread,
    LarkThread,
    FeishuThread,
}
#[derive(ToSchema)]
struct BridgeConnectorCapabilitiesShape {
    platform: String,
    evidence: BridgeCapabilityEvidenceShape,
    live_verified: bool,
    inbound_text: bool,
    stable_sender_chat: bool,
    stable_thread: Option<BridgeStableThreadShape>,
    reply_to_bot: bool,
    bot_mentions: bool,
    inbound_edits: bool,
    inbound_deletes: bool,
    verified_actions: bool,
    attachment_input: BridgeAttachmentInputShape,
    attachment_output: bool,
    output_text: bool,
    output_edit: bool,
    output_progress: bool,
    output_limit_utf16: Option<u32>,
    output_format: BridgeOutputFormatShape,
    stages: BridgeCapabilityStagesShape,
}
#[derive(ToSchema)]
struct BridgeCapabilityStagesShape {
    management: bool,
    ingress: bool,
    allocation: bool,
    execution: bool,
    outbound: bool,
}
