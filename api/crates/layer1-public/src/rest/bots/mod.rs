//! Public projection of durable owner Bot operations.
pub(crate) mod shapes;
use crate::AuthenticatedCaller;
use crate::DelegatedCredential;
use axum::{
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{bots::*, Caller};
use shapes::*;
use std::sync::Arc;

pub fn bot_router(control: Arc<BotControl>) -> Router {
    Router::new()
        .route("/api/v1/bots", get(list).post(create))
        .route("/api/v1/bots/nyxid-bots", get(available))
        .route("/api/v1/bots/connections", get(connections))
        .route("/api/v1/bots/{bot_id}", get(read).delete(delete))
        .route("/api/v1/bots/{bot_id}/test", post(test))
        .route("/api/v1/bots/{bot_id}/disable", post(disable))
        .route("/api/v1/bots/{bot_id}/enable", post(enable))
        .route("/api/v1/bots/{bot_id}/rotate-keys", post(rotate))
        .with_state(control)
        .layer(axum::middleware::from_fn(no_store))
}
async fn no_store(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
#[utoipa::path(post, path="/api/v1/bots", tag="bots", params(("Idempotency-Key"=String, Header)), request_body=CreateBotBody,
    responses((status=201, body=BotView), (status=400, body=super::ProblemDocument), (status=403, body=super::ProblemDocument), (status=409, body=super::ProblemDocument), (status=422, body=super::ProblemDocument), (status=503, body=super::ProblemDocument)))]
pub async fn create(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<Extension<DelegatedCredential>>,
    headers: HeaderMap,
    body: Result<Json<CreateBotBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(error) => return super::BodyProblem::from(error).into_response(),
    };
    let key = match headers
        .get_all("idempotency-key")
        .iter()
        .collect::<Vec<_>>()
        .as_slice()
    {
        [key] => key.to_str().unwrap_or_default(),
        _ => "",
    };
    let result = match body.into_core() {
        Ok(body) => {
            control
                .service
                .create_with_credential(
                    &caller,
                    credential.as_ref().map(|value| value.value()),
                    key,
                    body,
                )
                .await
        }
        Err(error) => Err(error),
    };
    view(result, StatusCode::CREATED)
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    after: Option<String>,
    limit: Option<usize>,
}
#[utoipa::path(get, path="/api/v1/bots", tag="bots", params(ListQuery), responses((status=200, body=BotPage)))]
pub async fn list(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Query(query): Query<ListQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50);
    match control
        .service
        .list(&caller, query.after.as_deref(), limit)
        .await
    {
        Ok(bots) => {
            let next_after = (bots.len() == limit)
                .then(|| bots.last().map(|b| b.bot_id.clone()))
                .flatten();
            Json(BotPage {
                items: bots.into_iter().map(Into::into).collect(),
                next_after,
            })
            .into_response()
        }
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path="/api/v1/bots/nyxid-bots", tag="bots", responses((status=200, body=Vec<ExternalBotView>)))]
pub async fn available(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Response {
    match control.available(&caller).await {
        Ok(bots) => Json(
            bots.into_iter()
                .map(|b| ExternalBotView {
                    id: b.id,
                    name: b.name,
                    platform: b.platform,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path="/api/v1/bots/connections", tag="bots", responses((status=200, body=Vec<BotConnectionView>)))]
pub async fn connections(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Response {
    match control.available(&caller).await {
        Ok(connections) => Json(
            connections
                .into_iter()
                .map(|connection| BotConnectionView {
                    connection_ref: connection.id,
                    label: connection.name,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => problem(error),
    }
}
#[utoipa::path(get, path="/api/v1/bots/{bot_id}", tag="bots", params(("bot_id"=String, Path)), responses((status=200, body=BotView), (status=404, body=super::ProblemDocument)))]
pub async fn read(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    view(control.service.get(&caller, &id).await, StatusCode::OK)
}
#[utoipa::path(post, path="/api/v1/bots/{bot_id}/test", tag="bots", params(("bot_id"=String, Path)), responses((status=200, body=TestView)))]
pub async fn test(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    match control.test(&caller, &id).await {
        Ok(result) => Json(TestView::from(result)).into_response(),
        Err(error) => problem(error),
    }
}
#[utoipa::path(post, path="/api/v1/bots/{bot_id}/disable", tag="bots", params(("bot_id"=String, Path)), responses((status=202, body=BotView)))]
pub async fn disable(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    admit(control, caller, id, BotOperation::Disable).await
}
#[utoipa::path(post, path="/api/v1/bots/{bot_id}/enable", tag="bots", params(("bot_id"=String, Path)), responses((status=202, body=BotView)))]
pub async fn enable(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    admit(control, caller, id, BotOperation::Enable).await
}
#[utoipa::path(post, path="/api/v1/bots/{bot_id}/rotate-keys", tag="bots", params(("bot_id"=String, Path)), responses((status=202, body=BotView)))]
pub async fn rotate(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    admit(control, caller, id, BotOperation::Rotate).await
}
#[utoipa::path(delete, path="/api/v1/bots/{bot_id}", tag="bots", params(("bot_id"=String, Path)), responses((status=202, body=BotView)))]
pub async fn delete(
    State(control): State<Arc<BotControl>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    admit(control, caller, id, BotOperation::Delete).await
}
async fn admit(control: Arc<BotControl>, caller: Caller, id: String, op: BotOperation) -> Response {
    view(
        control.service.admit(&caller, &id, op).await,
        StatusCode::ACCEPTED,
    )
}
fn view(result: Result<Bot, BotError>, status: StatusCode) -> Response {
    match result {
        Ok(bot) => (status, Json(BotView::from(bot))).into_response(),
        Err(error) => problem(error),
    }
}
fn problem(error: BotError) -> Response {
    let (status, code) = match &error {
        BotError::AccountDeletionInProgress => {
            return super::account_deletion_problem(
                layer2_core::account_deletion::AccountDeletionError::InProgress,
            )
        }
        BotError::Forbidden => (StatusCode::FORBIDDEN, "bot_forbidden"),
        BotError::NotFound => (StatusCode::NOT_FOUND, "bot_not_found"),
        BotError::Conflict => (StatusCode::CONFLICT, "bot_conflict"),
        BotError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_bot"),
        BotError::ImageSelectionUnsupported => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "bot_image_selection_unsupported",
        ),
        BotError::ConnectionRequired { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "connection_required")
        }
        BotError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "bot_unavailable"),
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
    super::problems::problem_response(status, body)
}
