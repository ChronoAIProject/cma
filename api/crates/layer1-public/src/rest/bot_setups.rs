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
    bot_setups::{BotSetupService, SetupCustomization, SetupDraft},
    bots::BotError,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AdvanceSetup {
    pub event_ref: String,
    #[serde(default)]
    #[schema(value_type=Object)]
    pub draft: SetupDraft,
    #[serde(default)]
    #[schema(value_type=Object)]
    pub customization: SetupCustomization,
}

pub fn bot_setup_router(service: Arc<BotSetupService>) -> Router {
    Router::new()
        .route("/api/v1/bot-setups/advance", post(advance))
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
    if body.event_ref.len() > 16 * 1024 {
        return error(BotError::Invalid(
            "Event reference exceeds the permitted size.".into(),
        ));
    }
    let response = match service
        .advance(
            &binding_id,
            credential.value(),
            &body.event_ref,
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
fn error(error: BotError) -> Response {
    let (status, code) = match error {
        BotError::Forbidden => (StatusCode::FORBIDDEN, "setup_authority_required"),
        BotError::NotFound => (StatusCode::NOT_FOUND, "setup_not_found"),
        BotError::Conflict => (StatusCode::CONFLICT, "setup_changed"),
        BotError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_setup"),
        BotError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "setup_unavailable"),
    };
    super::problem_response(
        status,
        super::problem_document(status, code, &error.to_string(), code),
    )
}
