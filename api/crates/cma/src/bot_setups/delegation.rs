//! Model-facing CMA operations use event-bound user authority held by CMA.
mod policy;
use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use layer1_public::rest::openapi::agent_delegations::{
    AgentDelegationIdentityRequest as IdentityRequest,
    AgentDelegationOperationRequest as OperationRequest,
};
use layer1_public::{AuthMethod, AuthenticatedSubject, DelegatedCredential};
use layer2_core::{
    resource_access::ResourceOwner, ActivityAction, ActivityOutcome, ActivityRecord,
    ActivityTarget, DetailKey,
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[derive(Clone)]
struct DelegationState {
    dependencies: Arc<SetupAuthority>,
    resources: Router,
}

impl SetupDependencies {
    pub(crate) fn delegation_router(self: &Arc<Self>, resources: Router) -> Router {
        self.authority.delegation_router(resources)
    }
}
impl SetupAuthority {
    pub(super) fn delegation_router(self: &Arc<Self>, resources: Router) -> Router {
        Router::new()
            .route("/api/v1/agent-delegations/identity", post(identity))
            .route("/api/v1/agent-delegations/operations", post(operation))
            .with_state(DelegationState {
                dependencies: self.clone(),
                resources,
            })
    }
}

async fn identity(
    State(state): State<DelegationState>,
    Extension(method): Extension<AuthMethod>,
    Extension(credential): Extension<DelegatedCredential>,
    Json(body): Json<IdentityRequest>,
) -> Response {
    let AuthMethod::BindingKey { binding_id } = method;
    let result = async {
        let event = state
            .dependencies
            .verified_event(&binding_id, credential.value(), &body.event_ref, None)
            .await?;
        state.dependencies.link_identity(&event.actor).await
    }
    .await;
    no_store(match result {
        Ok(status) => Json(status).into_response(),
        Err(error) => problem(error),
    })
}

async fn operation(
    State(state): State<DelegationState>,
    Extension(method): Extension<AuthMethod>,
    Extension(credential): Extension<DelegatedCredential>,
    Json(body): Json<OperationRequest>,
) -> Response {
    let AuthMethod::BindingKey { binding_id } = method;
    no_store(
        match execute(&state, &binding_id, credential.value(), body).await {
            Ok(value) => Json(value).into_response(),
            Err(error) => problem(error),
        },
    )
}

async fn execute(
    state: &DelegationState,
    binding: &str,
    credential: &str,
    body: OperationRequest,
) -> Result<Value, BotError> {
    let method = Method::from_bytes(body.method.as_bytes())
        .map_err(|_| BotError::Invalid("Invalid method.".into()))?;
    policy::validate(&method, &body.path, body.idempotency_key.as_deref())?;
    let dependencies = &state.dependencies;
    let event = dependencies
        .verified_event(binding, credential, &body.event_ref, None)
        .await?;
    let identity = dependencies.link_identity(&event.actor).await?;
    let Some(grant_id) = identity.grant_id else {
        return Ok(
            json!({"status":401,"body":{"code":"user_login_required","login_url":identity.login_url}}),
        );
    };
    let actor = super::authorization::actor(&event.actor);
    let grant = dependencies
        .auth
        .current_actor_grant(&grant_id, &actor)
        .await?;
    if grant.scope != crate::auth::actor_links::ActorGrantScope::OrdinaryCma {
        return Err(BotError::Forbidden);
    }
    let owner = body.owner.unwrap_or(ResourceOwner::Person {
        subject: grant.subject.clone(),
    });
    policy::validate_owner(&method, &body.path, &owner)?;
    let (context, token) = dependencies
        .auth
        .context_for_grant(&grant_id, &actor, &owner)
        .await?;
    let request_bytes =
        serde_json::to_vec(&body.body.unwrap_or(json!({}))).map_err(|_| BotError::Unavailable)?;
    if request_bytes.len() > 256 * 1024 {
        return Err(BotError::Invalid("Operation body exceeds 256 KiB.".into()));
    }
    let mut request = Request::builder()
        .method(method.clone())
        .uri(&body.path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = &body.idempotency_key {
        request = request.header("idempotency-key", key);
    }
    let mut request = request
        .body(if method == Method::GET {
            Body::empty()
        } else {
            Body::from(request_bytes)
        })
        .map_err(|_| BotError::Invalid("Invalid operation request.".into()))?;
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(context.caller.subject.clone()));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(DelegatedCredential::new(token.expose().into()));
    // Persist delegation attribution before dispatch, including operations whose
    // legacy route records only the user. No input, cookie or token is logged.
    let mut audit = ActivityRecord::new(ActivityAction::Configure, ActivityOutcome::Succeeded)
        .by(grant.subject.clone())
        .on(ActivityTarget::Resource(
            body.path.split('?').next().unwrap_or_default().into(),
        ));
    audit
        .detail
        .record(DetailKey::AuthorizationGrantId, &grant_id);
    audit
        .detail
        .record(DetailKey::Status, "delegated_operation_admitted");
    audit.detail.record(
        DetailKey::CorrelationId,
        body.idempotency_key.as_deref().unwrap_or("read"),
    );
    dependencies
        .activity
        .append(&audit)
        .await
        .map_err(|_| BotError::Unavailable)?;
    let response = tokio::time::timeout(
        Duration::from_secs(45),
        state.resources.clone().oneshot(request),
    )
    .await
    .map_err(|_| BotError::Unavailable)?
    .map_err(|_| BotError::Unavailable)?;
    // An accepted response already has durable admission before its SSE body.
    // Return that receipt immediately; agents poll the ordinary GET response API.
    let result = if let Some(response_id) = response
        .headers()
        .get("x-cma-response-id")
        .and_then(|v| v.to_str().ok())
    {
        json!({"status":202,"body":{"response_id":response_id,"command_id":response.headers().get("x-cma-command-id").and_then(|v|v.to_str().ok())}})
    } else {
        let status = response.status().as_u16();
        let bytes = tokio::time::timeout(
            Duration::from_secs(10),
            to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .map_err(|_| BotError::Unavailable)?
        .map_err(|_| BotError::Unavailable)?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| BotError::Unavailable)?
        };
        json!({"status":status,"body":value})
    };
    let current = dependencies
        .auth
        .current_actor_grant(&grant_id, &actor)
        .await?;
    if current.subject != grant.subject
        || current.credential_incarnation != grant.credential_incarnation
    {
        return Err(BotError::Forbidden);
    }
    // The outer binding and original source event remain current as well.
    dependencies
        .admitted_event(
            &event.actor,
            &event.event_id,
            &dependencies.source_reference(&body.event_ref).await?,
            credential,
        )
        .await?;
    // Internal handler extensions hold credentials; the agent-facing result
    // must never copy them, even if a downstream handler accidentally echoes one.
    let serialized = serde_json::to_string(&result).map_err(|_| BotError::Unavailable)?;
    if serialized.contains(token.expose()) || serialized.contains(credential) {
        return Err(BotError::Unavailable);
    }
    Ok(result)
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
fn problem(error: BotError) -> Response {
    let status = match &error {
        BotError::AccountDeletionInProgress => {
            return no_store(layer1_public::rest::account_deletion_problem(
                layer2_core::account_deletion::AccountDeletionError::InProgress,
            ))
        }
        BotError::Forbidden => StatusCode::FORBIDDEN,
        BotError::NotFound => StatusCode::NOT_FOUND,
        BotError::Conflict => StatusCode::CONFLICT,
        BotError::Invalid(_) => StatusCode::BAD_REQUEST,
        BotError::ImageSelectionUnsupported => StatusCode::UNPROCESSABLE_ENTITY,
        BotError::ConnectionRequired { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        BotError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    let mut response = (
        status,
        Json(layer1_public::rest::ProblemDocument {
            problem_type: "https://spec.chrono-ai.fun/cma/1.0/problems/delegated-operation-refused"
                .into(),
            title: "delegated operation refused".into(),
            status: status.as_u16(),
            detail: error.to_string(),
            code: "delegated_operation_refused".into(),
        }),
    )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/problem+json"),
    );
    response
}
