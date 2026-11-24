//! A public Bot configuration workflow for authenticated provider integrations.
use crate::{AuthMethod, DelegatedCredential};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Extension, Json, Router,
};
use layer2_core::{
    bot_setups::{BotSetupService, SetupActionRequest, SetupCustomization, SetupDraft},
    bots::BotError,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AdvanceSetup {
    pub event_ref: String,
    /// Full ingress activity; the gateway attests it against the sealed reference.
    #[serde(default)]
    pub activity: Option<serde_json::Value>,
    #[serde(default)]
    #[schema(value_type=Object)]
    pub draft: SetupDraft,
    #[serde(default)]
    #[schema(value_type=Object)]
    pub customization: SetupCustomization,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentSetupAction {
    pub event_ref: String,
    #[schema(value_type=Object)]
    pub request: SetupActionRequest,
}

pub fn bot_setup_router(service: Arc<BotSetupService>) -> Router {
    Router::new()
        .route("/api/v1/bot-setups/advance", post(advance))
        .route("/api/v1/bot-setups/actions", post(act))
        .with_state(service)
}

#[utoipa::path(post,path="/api/v1/bot-setups/advance",tag="bots",request_body=AdvanceSetup,responses((status=200,body=Object),(status=400),(status=401),(status=403),(status=409),(status=503)))]
pub(super) async fn advance(
    State(service): State<Arc<BotSetupService>>,
    Extension(method): Extension<AuthMethod>,
    Extension(credential): Extension<DelegatedCredential>,
    Json(body): Json<AdvanceSetup>,
) -> Response {
    let AuthMethod::BindingKey { binding_id } = method;
    if body.event_ref.len() > 4096 {
        return error(BotError::Invalid(
            "Event reference exceeds the permitted size.".into(),
        ));
    }
    let response = match service
        .advance_with_activity(
            &binding_id,
            credential.value(),
            &body.event_ref,
            body.activity.as_ref(),
            body.draft,
            body.customization,
        )
        .await
    {
        Ok(view) => Json(view).into_response(),
        Err(problem) => error(problem),
    };
    ([(header::CACHE_CONTROL, "no-store")], response).into_response()
}
#[utoipa::path(post,path="/api/v1/bot-setups/actions",tag="bots",request_body=AgentSetupAction,responses((status=200,body=Object),(status=400),(status=401),(status=403),(status=409),(status=503)))]
pub(super) async fn act(
    State(service): State<Arc<BotSetupService>>,
    Extension(method): Extension<AuthMethod>,
    Extension(credential): Extension<DelegatedCredential>,
    Json(body): Json<AgentSetupAction>,
) -> Response {
    let AuthMethod::BindingKey { binding_id } = method;
    if body.event_ref.len() > 4096 {
        return error(BotError::Invalid(
            "Event reference exceeds the permitted size.".into(),
        ));
    }
    let response = match service
        .act(
            &binding_id,
            credential.value(),
            &body.event_ref,
            body.request,
        )
        .await
    {
        Ok(view) => Json(view).into_response(),
        Err(problem) => error(problem),
    };
    ([(header::CACHE_CONTROL, "no-store")], response).into_response()
}

fn error(error: BotError) -> Response {
    let (status, code) = match &error {
        BotError::AccountDeletionInProgress => {
            return super::account_deletion_problem(
                layer2_core::account_deletion::AccountDeletionError::InProgress,
            )
        }
        BotError::Forbidden => (StatusCode::FORBIDDEN, "setup_authority_required"),
        BotError::NotFound => (StatusCode::NOT_FOUND, "setup_not_found"),
        BotError::Conflict => (StatusCode::CONFLICT, "setup_changed"),
        BotError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_setup"),
        BotError::ImageSelectionUnsupported => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "bot_image_selection_unsupported",
        ),
        BotError::ConnectionRequired { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "connection_required")
        }
        BotError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "setup_unavailable"),
    };
    let mut body = super::problem_document(status, code, &error.to_string(), code);
    if let BotError::ConnectionRequired {
        services,
        connection_target,
    } = error
    {
        if let Some(object) = body.as_object_mut() {
            object.insert(
                "services".into(),
                serde_json::to_value(services).expect("service identifiers are JSON values"),
            );
            object.insert("connection_target".into(), connection_target);
        }
    }
    super::problem_response(status, body)
}
