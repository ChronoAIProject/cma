use super::*;
use layer2_core::sandbox::agent_environment::{
    BrowserObservationView, BrowserSelection, BrowserState,
};

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(read, configure, release),
    components(schemas(BrowserView, BrowserFrameView, BrowserEnablement))
)]
struct BrowserApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    BrowserApi::openapi()
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserQuery {
    runtime_incarnation: String,
    observer: Option<String>,
    context_id: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserEnablement {
    runtime_incarnation: String,
    enabled: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct BrowserFrameView {
    sequence: u64,
    captured_at: String,
    media_type: String,
    width: u32,
    height: u32,
    data: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct BrowserView {
    agent_id: String,
    runtime_incarnation: String,
    state: &'static str,
    context_id: Option<String>,
    location: String,
    frame: Option<BrowserFrameView>,
    observers: u32,
    captures: u64,
    bytes: u64,
    min_interval_ms: u32,
    fresh_for_ms: u32,
    max_frame_bytes: u32,
    max_observers: u32,
}

impl From<BrowserObservationView> for BrowserView {
    fn from(value: BrowserObservationView) -> Self {
        let observed = value.observation;
        Self {
            agent_id: value.agent_id,
            runtime_incarnation: value.runtime_incarnation,
            state: match observed.state {
                BrowserState::NotEnabled => "not_enabled",
                BrowserState::Unsupported => "unsupported",
                BrowserState::Loading => "loading",
                BrowserState::Ready => "ready",
                BrowserState::Live => "live",
                BrowserState::Paused => "paused",
                BrowserState::Disconnected => "disconnected",
            },
            context_id: observed.context_id,
            location: observed.location,
            frame: observed.frame.map(|frame| BrowserFrameView {
                sequence: frame.sequence,
                captured_at: frame.captured_at,
                media_type: frame.media_type,
                width: frame.width,
                height: frame.height,
                data: frame.data,
            }),
            observers: observed.observers,
            captures: observed.captures,
            bytes: observed.bytes,
            min_interval_ms: observed.min_interval_ms,
            fresh_for_ms: observed.fresh_for_ms,
            max_frame_bytes: observed.max_frame_bytes,
            max_observers: observed.max_observers,
        }
    }
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/browser", tag = "agents",
    params(("id" = String, Path), ("runtime_incarnation" = String, Query), ("observer" = Option<String>, Query), ("context_id" = Option<String>, Query)),
    responses((status = 200, body = BrowserView)))]
pub(super) async fn read(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    Query(query): Query<BrowserQuery>,
) -> Result<Json<BrowserView>, FileProblem> {
    Ok(Json(
        service
            .browser_observation(
                &caller,
                context(&resource),
                credential(&resource),
                &AgentId(id),
                BrowserSelection {
                    runtime_incarnation: query.runtime_incarnation,
                    observer: query.observer,
                    context_id: query.context_id,
                    enabled: None,
                    release: false,
                },
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(put, path = "/api/v1/agents/{id}/browser", tag = "agents",
    params(("id" = String, Path)), request_body = BrowserEnablement, responses((status = 200, body = BrowserView)))]
pub(super) async fn configure(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    Json(input): Json<BrowserEnablement>,
) -> Result<Json<BrowserView>, FileProblem> {
    Ok(Json(
        service
            .browser_observation(
                &caller,
                context(&resource),
                credential(&resource),
                &AgentId(id),
                BrowserSelection {
                    runtime_incarnation: input.runtime_incarnation,
                    observer: None,
                    context_id: None,
                    enabled: Some(input.enabled),
                    release: false,
                },
            )
            .await?
            .into(),
    ))
}

#[utoipa::path(delete, path = "/api/v1/agents/{id}/browser", tag = "agents",
    params(("id" = String, Path), ("runtime_incarnation" = String, Query), ("observer" = String, Query)),
    responses((status = 200, body = BrowserView)))]
pub(super) async fn release(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    Query(query): Query<BrowserQuery>,
) -> Result<Json<BrowserView>, FileProblem> {
    Ok(Json(
        service
            .browser_observation(
                &caller,
                context(&resource),
                credential(&resource),
                &AgentId(id),
                BrowserSelection {
                    runtime_incarnation: query.runtime_incarnation,
                    observer: query.observer,
                    context_id: query.context_id,
                    enabled: None,
                    release: true,
                },
            )
            .await?
            .into(),
    ))
}
