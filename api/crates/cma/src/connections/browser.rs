//! Browser authentication coordination. NyxID retains its hosted handoffs;
//! native subscription intents are durable in CMA. No resource is allocated here.

use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use layer1_public::{AuthenticatedCaller, AuthenticatedResourceCaller};
use layer2_core::{
    account_deletion::OwnerSessionAuthority,
    sandbox::SandboxPolicySource,
    trigger_domain::{
        BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionMode, ServiceRequirementState,
    },
    Caller, ServiceId,
};
use layer3_adapter::codex::{
    CODEX_SUBSCRIPTION_CONNECTION, CONNECTION_SERVICES, PUBLIC_CODEX_SERVICE,
};
use serde::{Deserialize, Serialize};

use crate::{
    auth::{service_authorization, AuthState},
    store::MongoStores,
};

use super::{
    targets::{ConnectionTarget, ConnectionTargets, StoredConnectionTargets},
    CodexSubscriptions,
};

mod repository_access;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionError {
    Forbidden,
    NotFound,
    Invalid,
    Unavailable,
    Changed,
}

impl IntoResponse for ConnectionError {
    fn into_response(self) -> Response {
        let (status, code, detail) = match self {
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "This account cannot connect services for this operation.",
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "connection_target_not_found",
                "The selected sandbox, profile or session is no longer available.",
            ),
            Self::Invalid => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_connection_request",
                "This service is not required by the selected sandbox.",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "connection_unavailable",
                "The connection could not be verified. Please try again.",
            ),
            Self::Changed => (
                StatusCode::CONFLICT,
                "connection_changed",
                "This connection attempt has changed. Please reconnect to continue.",
            ),
        };
        (status, [(header::CONTENT_TYPE, "application/problem+json"), (header::CACHE_CONTROL, "private, no-store")], Json(serde_json::json!({
            "type":format!("https://spec.chrono-ai.fun/cma/1.0/problems/{code}"),
            "title":"Connection could not continue", "status":status.as_u16(), "code":code, "detail":detail,
        }))).into_response()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckRequest {
    target: ConnectionTarget,
    #[serde(default)]
    active: Option<ActiveConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveConnection {
    service_id: ServiceId,
    intent_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    target: ConnectionTarget,
    service_id: ServiceId,
    intent_id: String,
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
struct ConnectionRow {
    service: ServiceId,
    display_name: String,
    kind: &'static str,
    state: ServiceRequirementState,
    connection_mode: &'static str,
}

#[derive(Serialize)]
struct ConnectionReport {
    owner: String,
    ready: bool,
    checked_at: String,
    services: Vec<ConnectionRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_state: Option<BrokerHandoffState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_authorization_recovery: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationRequest {
    target: ConnectionTarget,
    return_to: String,
}

#[derive(Serialize)]
struct StartedConnection {
    owner: String,
    service_id: ServiceId,
    intent_id: String,
    hosted_url: String,
    expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_code: Option<String>,
}

pub(crate) struct BrowserConnections {
    targets: Arc<dyn ConnectionTargets>,
    broker: Arc<dyn ServiceConnectionBroker>,
    subscriptions: Arc<CodexSubscriptions>,
    repository_access: Option<Arc<crate::trigger::CmaServiceConnectionBroker>>,
    repository_scope_check: bool,
    github_connection_approval: bool,
}

impl BrowserConnections {
    pub(crate) fn new(
        stores: Arc<MongoStores>,
        policy: Arc<dyn SandboxPolicySource>,
        broker: Arc<dyn ServiceConnectionBroker>,
        subscriptions: Arc<CodexSubscriptions>,
        repository_access: Option<Arc<crate::trigger::CmaServiceConnectionBroker>>,
        repository_scope_check: bool,
        github_connection_approval: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            targets: Arc::new(StoredConnectionTargets { stores, policy }),
            broker,
            subscriptions,
            repository_access,
            repository_scope_check,
            github_connection_approval,
        })
    }

    async fn check(
        &self,
        caller: &Caller,
        request: CheckRequest,
    ) -> Result<ConnectionReport, ConnectionError> {
        let started = std::time::Instant::now();
        let services = self.targets.services(caller, &request.target).await;
        check_timing("target_authorization", started, services.is_ok());
        let services = services?;
        // A ready Agent needs fresh requirements, but no catalog or repair choices.
        // Do not start metadata speculatively: resolving its credential can refresh
        // OAuth custody, which must not be cancelled by a successful early return.
        let ordinary_agent =
            matches!(&request.target, ConnectionTarget::Agent { .. }) && request.active.is_none();
        let requirements_read = async {
            let started = std::time::Instant::now();
            let result = self.broker.requirements(&caller.subject, &services).await;
            check_timing("requirements", started, result.is_ok());
            result
        };
        let metadata_read = async {
            let started = std::time::Instant::now();
            let result = self
                .broker
                .describe_services(&caller.subject, &services)
                .await;
            check_timing("metadata", started, result.is_ok());
            result
        };
        tokio::pin!(metadata_read);
        let (requirements, completed_metadata) = if ordinary_agent {
            (requirements_read.await, None)
        } else {
            let (requirements, metadata) = tokio::join!(requirements_read, &mut metadata_read);
            (requirements, Some(metadata))
        };
        let requirements = requirements.map_err(|_| ConnectionError::Unavailable)?;
        if requirements.len() != services.len()
            || !services
                .iter()
                .eq(requirements.iter().map(|requirement| &requirement.service))
        {
            return Err(ConnectionError::Unavailable);
        }
        let ready_agent = ordinary_agent
            && requirements
                .iter()
                .all(|row| row.state == ServiceRequirementState::Satisfied);
        let metadata = if ready_agent {
            // The existing unknown metadata projection makes no repair promise.
            // It never substitutes for the fresh service states validated above.
            Vec::new()
        } else {
            match completed_metadata {
                Some(result) => result,
                None => metadata_read.await,
            }
            .map_err(|_| ConnectionError::Unavailable)?
        };
        let mut seen = BTreeSet::new();
        if metadata
            .iter()
            .any(|item| !services.contains(&item.service) || !seen.insert(&item.service))
        {
            return Err(ConnectionError::Unavailable);
        }
        let active_state = if let Some(active) = request
            .active
            .as_ref()
            .filter(|active| active.service_id.as_str() == CODEX_SUBSCRIPTION_CONNECTION)
        {
            if !services.contains(&active.service_id) {
                return Err(ConnectionError::Invalid);
            }
            Some(
                self.subscriptions
                    .status(&caller.subject, &active.intent_id)
                    .await
                    .map_err(|_| ConnectionError::Changed)?
                    .state,
            )
        } else {
            None
        };
        let rows = requirements
            .into_iter()
            .map(|requirement| {
                let descriptor = metadata
                    .iter()
                    .find(|item| item.service == requirement.service);
                let is_model = CONNECTION_SERVICES.contains(requirement.service.as_str());
                let display_name = match requirement.service.as_str() {
                    CODEX_SUBSCRIPTION_CONNECTION => "My Codex subscription".to_owned(),
                    PUBLIC_CODEX_SERVICE => "Chrono LLM (public)".to_owned(),
                    _ => descriptor
                        .and_then(|item| item.display_name.clone())
                        .unwrap_or_else(|| "Required NyxID service".into()),
                };
                let state = if request
                    .active
                    .as_ref()
                    .is_some_and(|active| active.service_id == requirement.service)
                    && active_state.is_some_and(|state| state != BrokerHandoffState::Satisfied)
                {
                    ServiceRequirementState::Missing
                } else {
                    requirement.state
                };
                ConnectionRow {
                    service: requirement.service,
                    display_name,
                    kind: if is_model { "codex" } else { "nyxid" },
                    state,
                    connection_mode: match descriptor.map(|item| item.connection_mode) {
                        Some(ServiceConnectionMode::Hosted) => "hosted",
                        Some(ServiceConnectionMode::Manual) => "manual",
                        _ => "unknown",
                    },
                }
            })
            .collect::<Vec<_>>();
        Ok(ConnectionReport {
            owner: caller.subject.to_string(),
            ready: rows
                .iter()
                .all(|row| row.state == ServiceRequirementState::Satisfied),
            checked_at: layer2_core::now_rfc3339(),
            services: rows,
            active_state,
            service_authorization_recovery: None,
        })
    }

    async fn start(
        &self,
        caller: &Caller,
        request: StartRequest,
    ) -> Result<StartedConnection, ConnectionError> {
        if uuid::Uuid::parse_str(&request.intent_id).is_err() {
            return Err(ConnectionError::Invalid);
        }
        let services = self.targets.services(caller, &request.target).await?;
        if !services.contains(&request.service_id)
            || (request.force && request.service_id.as_str() != CODEX_SUBSCRIPTION_CONNECTION)
        {
            return Err(ConnectionError::Invalid);
        }
        let metadata = self
            .broker
            .describe_services(&caller.subject, std::slice::from_ref(&request.service_id))
            .await
            .map_err(|_| ConnectionError::Unavailable)?;
        let descriptor = metadata
            .into_iter()
            .find(|item| item.service == request.service_id)
            .ok_or(ConnectionError::Unavailable)?;
        let (hosted_url, expires_at, user_code) = match descriptor.connection_mode {
            ServiceConnectionMode::Hosted => {
                let handoff = self
                    .broker
                    .create_handoff(&caller.subject, &request.service_id, &request.intent_id)
                    .await
                    .map_err(|_| ConnectionError::Unavailable)?;
                let code = if request.service_id.as_str() == CODEX_SUBSCRIPTION_CONNECTION {
                    Some(
                        self.subscriptions
                            .status(&caller.subject, &request.intent_id)
                            .await
                            .map_err(|_| ConnectionError::Changed)?
                            .device
                            .user_code,
                    )
                } else {
                    None
                };
                (handoff.hosted_url, handoff.expires_at, code)
            }
            ServiceConnectionMode::Manual => (
                descriptor.setup_url.ok_or(ConnectionError::Unavailable)?,
                layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(15 * 60)),
                None,
            ),
            ServiceConnectionMode::Unknown => return Err(ConnectionError::Unavailable),
        };
        let url = reqwest::Url::parse(&hosted_url).map_err(|_| ConnectionError::Unavailable)?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(ConnectionError::Unavailable);
        }
        Ok(StartedConnection {
            owner: caller.subject.to_string(),
            service_id: request.service_id,
            intent_id: request.intent_id,
            hosted_url,
            expires_at,
            user_code,
        })
    }
}

pub(crate) fn router(state: Arc<BrowserConnections>, auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/connections/check", post(check))
        .route(
            "/auth/connections/repository-access",
            post(repository_access::report),
        )
        .route("/auth/connections/start", post(start))
        .route("/auth/connections/cancel", post(cancel))
        .route(
            "/auth/service-authorization/start",
            post(start_authorization),
        )
        .with_state(state)
        .layer(Extension(auth))
}

async fn check(
    State(state): State<Arc<BrowserConnections>>,
    Extension(auth): Extension<Arc<AuthState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<CheckRequest>,
) -> Result<Response, ConnectionError> {
    let mut report = state.check(&caller, request).await?;
    if auth.service_authorization_recovery_available()
        && report.services.iter().any(|row| row.kind == "nyxid")
    {
        report.service_authorization_recovery = Some(true);
    }
    Ok(no_store(report))
}

async fn start_authorization(
    State(state): State<Arc<BrowserConnections>>,
    Extension(auth): Extension<Arc<AuthState>>,
    AuthenticatedResourceCaller { context, .. }: AuthenticatedResourceCaller,
    authority: Option<Extension<OwnerSessionAuthority>>,
    headers: HeaderMap,
    Json(request): Json<AuthorizationRequest>,
) -> Response {
    if let Err(error) =
        service_authorization::start_allowed(&auth, &headers, context.credential_source)
    {
        return error.into_response();
    }
    if authority.is_none_or(|Extension(authority)| authority.subject() != &context.caller.subject) {
        return service_authorization::RecoveryError::Forbidden.into_response();
    }
    let services = match state
        .targets
        .services(&context.caller, &request.target)
        .await
    {
        Ok(services) => services,
        Err(error) => return error.into_response(),
    };
    service_authorization::begin(
        &auth,
        &headers,
        &context.caller.subject,
        services
            .into_iter()
            .filter(|service| service.as_str() != CODEX_SUBSCRIPTION_CONNECTION)
            .collect(),
        &request.return_to,
    )
    .await
    .into_response()
}

async fn start(
    State(state): State<Arc<BrowserConnections>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<StartRequest>,
) -> Result<Response, ConnectionError> {
    Ok(no_store(state.start(&caller, request).await?))
}

async fn cancel(
    State(state): State<Arc<BrowserConnections>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<CheckRequest>,
) -> Result<Response, ConnectionError> {
    let services = state.targets.services(&caller, &request.target).await?;
    let active = request.active.ok_or(ConnectionError::Invalid)?;
    if !services.contains(&active.service_id) {
        return Err(ConnectionError::Invalid);
    }
    if active.service_id.as_str() == CODEX_SUBSCRIPTION_CONNECTION {
        state
            .subscriptions
            .cancel(&caller.subject, &active.intent_id)
            .await
            .map_err(|_| ConnectionError::Changed)?;
    }
    Ok(no_store(serde_json::json!({"cancelled":true})))
}

fn no_store(body: impl Serialize) -> Response {
    ([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response()
}

fn check_timing(phase: &'static str, started: std::time::Instant, success: bool) {
    tracing::info!(
        phase,
        elapsed_ms = started.elapsed().as_millis() as u64,
        success,
        "Connection check phase completed"
    );
}

#[cfg(test)]
#[path = "browser_tests.rs"]
pub(super) mod tests;
