use super::*;
use layer2_core::sandbox::agent_browser_control::{
    BrowserControlCommand, BrowserControlSelection, BrowserControlState,
    BrowserControlView as CoreView, BrowserInput,
};

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(read, change),
    components(schemas(
        ControlQuery,
        ControlChange,
        ControlInput,
        ControlView,
        ControlAuditView
    ))
)]
struct ControlApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    ControlApi::openapi()
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlQuery {
    runtime_incarnation: String,
    context_id: String,
    client_id: String,
    operation_id: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlChange {
    runtime_incarnation: String,
    context_id: String,
    client_id: String,
    command: String,
    operation_id: String,
    expected_revision: u64,
    lease_token: Option<String>,
    input: Option<ControlInput>,
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ControlInput {
    Click { x: u16, y: u16 },
    Text { text: String },
    Key { key: String },
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ControlAuditView {
    revision: u64,
    kind: String,
    actor: String,
    client_id: String,
    operation_id: Option<String>,
    at: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ControlView {
    agent_id: String,
    runtime_incarnation: String,
    context_id: String,
    state: &'static str,
    revision: u64,
    client_id: Option<String>,
    lease_token: Option<String>,
    expires_at: Option<String>,
    lease_duration_ms: u32,
    operation_id: Option<String>,
    operation_revision: Option<u64>,
    audit: Vec<ControlAuditView>,
    audit_truncated_before: Option<u64>,
}

impl From<CoreView> for ControlView {
    fn from(value: CoreView) -> Self {
        let control = value.control;
        Self {
            agent_id: value.agent_id,
            runtime_incarnation: value.runtime_incarnation,
            context_id: value.context_id,
            state: match control.state {
                BrowserControlState::Agent => "agent",
                BrowserControlState::User => "user",
                BrowserControlState::Unknown => "unknown",
            },
            revision: control.revision,
            client_id: control.client_id,
            lease_token: control.lease_token,
            expires_at: control.expires_at,
            lease_duration_ms: control.lease_duration_ms,
            operation_id: control.operation_id,
            operation_revision: control.operation_revision,
            audit_truncated_before: control.audit_truncated_before,
            audit: control
                .audit
                .into_iter()
                .map(|event| ControlAuditView {
                    revision: event.revision,
                    kind: event.kind,
                    actor: event.actor,
                    client_id: event.client_id,
                    operation_id: event.operation_id,
                    at: event.at,
                })
                .collect(),
        }
    }
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/browser/control", tag = "agents",
    params(("id" = String, Path), ("runtime_incarnation" = String, Query), ("context_id" = String, Query),
        ("client_id" = String, Query), ("operation_id" = Option<String>, Query), ("x-cma-browser-client" = Option<String>, Header)),
    responses((status = 200, body = ControlView)))]
pub(super) async fn read(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    query: Result<Query<ControlQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ControlView>, ControlProblem> {
    let Query(query) = query.map_err(|_| ControlProblem(AgentFileError::Invalid))?;
    let value = service
        .browser_control(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            BrowserControlSelection {
                runtime_incarnation: query.runtime_incarnation,
                context_id: query.context_id,
                client_id: query.client_id,
                client_key: headers
                    .get("x-cma-browser-client")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                command: BrowserControlCommand::Status,
                operation_id: query.operation_id,
                expected_revision: None,
                lease_token: None,
                input: None,
            },
        )
        .await?;
    Ok(Json(value.into()))
}

#[utoipa::path(post, path = "/api/v1/agents/{id}/browser/control", tag = "agents",
    params(("id" = String, Path), ("x-cma-browser-client" = String, Header)), request_body = ControlChange, responses((status = 200, body = ControlView)))]
pub(super) async fn change(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    input: Result<Json<ControlChange>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ControlView>, ControlProblem> {
    let Json(input) = input.map_err(|_| ControlProblem(AgentFileError::Invalid))?;
    let command = match input.command.as_str() {
        "acquire" => BrowserControlCommand::Acquire,
        "input" => BrowserControlCommand::Input,
        "renew" => BrowserControlCommand::Renew,
        "return" => BrowserControlCommand::Return,
        _ => return Err(ControlProblem(AgentFileError::Invalid)),
    };
    let value = service
        .browser_control(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
            BrowserControlSelection {
                runtime_incarnation: input.runtime_incarnation,
                context_id: input.context_id,
                client_id: input.client_id,
                command,
                client_key: headers
                    .get("x-cma-browser-client")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                operation_id: Some(input.operation_id),
                expected_revision: Some(input.expected_revision),
                lease_token: input.lease_token,
                input: input.input.map(|value| match value {
                    ControlInput::Click { x, y } => BrowserInput::Click { x, y },
                    ControlInput::Text { text } => BrowserInput::Text { text },
                    ControlInput::Key { key } => BrowserInput::Key { key },
                }),
            },
        )
        .await?;
    Ok(Json(value.into()))
}

pub(super) struct ControlProblem(AgentFileError);
impl From<AgentFileError> for ControlProblem {
    fn from(value: AgentFileError) -> Self {
        Self(value)
    }
}
impl IntoResponse for ControlProblem {
    fn into_response(self) -> Response {
        if let AgentFileError::Access(error) = self.0 {
            return super::super::resource_access_problem::ResourceProblem(error).into_response();
        }
        let (status, code, detail) = match self.0 {
            AgentFileError::RuntimeCapacity(capacity) => return crate::rest::problems::runtime_capacity(capacity),
            AgentFileError::Invalid | AgentFileError::TooLarge | AgentFileError::NameTooLong => (StatusCode::BAD_REQUEST, "invalid_browser_control", "Browser control request is invalid."),
            AgentFileError::Unsupported => (StatusCode::CONFLICT, "browser_control_unsupported", "This runtime has no verified browser control capability."),
            AgentFileError::NotReady => (StatusCode::CONFLICT, "browser_control_not_ready", "The browser runtime is not active."),
            AgentFileError::Conflict => (StatusCode::CONFLICT, "browser_control_conflict", "Control identity changed or the operation cannot be confirmed. Query the original operation."),
            AgentFileError::NotFound => (StatusCode::NOT_FOUND, "browser_control_not_found", "Browser control is unavailable."),
            AgentFileError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "browser_control_unknown", "Control state is unknown. Query the original operation before sending input."),
            AgentFileError::Access(_) => unreachable!(),
        };
        super::super::problem_response(
            status,
            super::super::problem_document(status, code, detail, code),
        )
    }
}
