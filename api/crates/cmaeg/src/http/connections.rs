//! Authenticated platform-neutral connection operations.
use crate::{
    auth::{AgentCaller, Creator, CreatorAuthState, CreatorAuthenticator},
    connections::{model::*, ConnectionService, Error},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

impl CreatorAuthState for ConnectionService {
    fn creator_auth(&self) -> &CreatorAuthenticator {
        &self.management.creator_auth
    }
}
pub fn router(service: ConnectionService) -> Router {
    let router = Router::new()
        .route("/v1/channel-capabilities", get(capabilities))
        .route("/v1/channel-connections", post(create).get(list))
        .route(
            "/v1/channel-connections/{connection_ref}",
            get(read).delete(cancel),
        )
        .with_state(service)
        .layer(axum::middleware::from_fn(super::problems::protocol_headers));
    crate::telemetry::http::instrument(router)
}
#[derive(Serialize, ToSchema)]
pub struct CapabilityView {
    /// Server tools selected by the same runtime gates as MCP tools/list.
    /// This catalog does not grant channel, source or event permission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_tools: Option<Vec<crate::mcp::catalog::AgentToolCapability>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_features: Vec<String>,
    /// Native connection setup workflows; separate from NyxID relay ingestion.
    pub platforms: Vec<PlatformCapability>,
    /// Additive relay authoring discovery; older gateways omit this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_authoring: Option<super::relay_capabilities::RelayAuthoringCapabilities>,
}
#[derive(Serialize, ToSchema)]
pub struct PlatformCapability {
    pub id: String,
    pub label: String,
    pub connection_methods: Vec<String>,
    pub optional_capabilities: Vec<String>,
}
#[utoipa::path(get,path="/v1/channel-capabilities",operation_id="getChannelCapabilities",tag="connections",security(("creatorBearer"=[])),responses((status=200,body=CapabilityView)))]
pub async fn capabilities(State(service): State<ConnectionService>, _creator: Creator) -> Response {
    no_store(
        Json(CapabilityView {
            agent_tools: Some(crate::mcp::catalog::view(&service.tools)),
            source_features: {
                let mut features = if service.management.native_bot_mentions_writes_enabled {
                    vec!["native_bot_mentions_v1".into()]
                } else {
                    Vec::new()
                };
                if service.management.github.get().is_some() {
                    features.push("github_native_receipts_v1".into());
                }
                if let Some(capability) = service.management.providers.turn_trace_capability() {
                    features.push(capability.into());
                }
                if service
                    .tools
                    .routed_replies
                    .as_ref()
                    .is_some_and(|routed| routed.accepted_answer_receipts_enabled())
                {
                    features.push("accepted_answer_delivery_receipts_v1".into());
                }
                features
            },
            relay_authoring: Some(super::relay_capabilities::view(&service.management)),
            platforms: if service.management.managed_telegram_setup_enabled {
                vec![PlatformCapability {
                    id: "telegram".into(),
                    label: "Telegram".into(),
                    connection_methods: vec!["existing_credential".into(), "managed_create".into()],
                    optional_capabilities: vec!["manage_bots".into()],
                }]
            } else {
                Vec::new()
            },
        })
        .into_response(),
    )
}
#[utoipa::path(post,path="/v1/channel-connections",operation_id="createChannelConnection",tag="connections",security(("creatorBearer"=[])),params(("X-CMAEG-Owner"=Option<String>,Header),("X-CMAEG-Agent-Authorization"=String,Header,description="Bearer authorizing the originating event's channel")),request_body=CreateConnection,responses((status=201,body=ConnectionView),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn create(
    State(service): State<ConnectionService>,
    creator: Creator,
    headers: HeaderMap,
    body: Result<Json<CreateConnection>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return failure(Error::Invalid("invalid connection request"));
    };
    let mut values = headers.get_all("x-cmaeg-agent-authorization").iter();
    let Some(value) = values.next() else {
        return failure(Error::Invalid("originating agent authorization required"));
    };
    if values.next().is_some() {
        return failure(Error::Invalid("originating agent authorization required"));
    }
    let mut agent_headers = HeaderMap::new();
    agent_headers.insert("authorization", value.clone());
    let agent = match AgentCaller::from_headers(&agent_headers) {
        Ok(agent) => agent,
        Err(e) => return e.into_response(),
    };
    match service.create(&creator, agent, body).await {
        Ok(view) => no_store((StatusCode::CREATED, Json(view)).into_response()),
        Err(e) => failure(e),
    }
}
#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    after: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct ConnectionPage {
    pub items: Vec<ConnectionView>,
    pub next_after: Option<String>,
}
#[utoipa::path(get,path="/v1/channel-connections",operation_id="listChannelConnections",tag="connections",security(("creatorBearer"=[])),params(ListQuery,("X-CMAEG-Owner"=Option<String>,Header)),responses((status=200,body=ConnectionPage),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn list(
    State(service): State<ConnectionService>,
    creator: Creator,
    Query(query): Query<ListQuery>,
) -> Response {
    match service.list(&creator, query.after.as_deref()).await {
        Ok((items, next_after)) => {
            no_store(Json(ConnectionPage { items, next_after }).into_response())
        }
        Err(e) => failure(e),
    }
}
#[utoipa::path(get,path="/v1/channel-connections/{connection_ref}",operation_id="getChannelConnection",tag="connections",security(("creatorBearer"=[])),params(("connection_ref"=String,Path),("X-CMAEG-Owner"=Option<String>,Header)),responses((status=200,body=ConnectionView),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn read(
    State(service): State<ConnectionService>,
    creator: Creator,
    Path(id): Path<String>,
) -> Response {
    match service.get(&creator, &id).await {
        Ok(view) => no_store(Json(view).into_response()),
        Err(e) => failure(e),
    }
}
#[utoipa::path(delete,path="/v1/channel-connections/{connection_ref}",operation_id="cancelChannelConnection",tag="connections",security(("creatorBearer"=[])),params(("connection_ref"=String,Path),("X-CMAEG-Owner"=Option<String>,Header)),responses((status=204),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn cancel(
    State(service): State<ConnectionService>,
    creator: Creator,
    Path(id): Path<String>,
) -> Response {
    match service.cancel(&creator, &id).await {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(e) => failure(e),
    }
}
pub(super) fn failure(error: Error) -> Response {
    let error = error.problem();
    no_store(super::problems::problem(
        StatusCode::from_u16(error.status).unwrap(),
        error.code,
        error.detail.unwrap_or_default(),
    ))
}
fn no_store(response: Response) -> Response {
    super::problems::no_store(response)
}
