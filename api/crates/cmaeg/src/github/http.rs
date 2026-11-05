use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};

use super::{BindRequest, Error};
use crate::{auth::IntegrationOwner, http::ManagementState};

pub fn routes() -> Router<ManagementState> {
    Router::new()
        .route("/v1/github/capabilities", get(capabilities))
        .route("/v1/github/apps", get(list_apps).post(create_app))
        .route("/v1/github/apps/{app_ref}", get(read_app).patch(update_app))
        .route("/v1/github/apps/{app_ref}/verification", post(verify_app))
        .route(
            "/v1/github/apps/{app_ref}/webhook-secret",
            put(webhook_secret),
        )
        .route("/v1/github/installation-bindings", post(bind))
        .route(
            "/v1/github/installation-bindings/{binding_id}",
            get(binding),
        )
        .route("/v1/github/apps/{app_ref}/webhooks", post(webhook))
        .route("/v1/github/deliveries/{receipt_id}", get(receipt))
        .route("/v1/github/tasks/{task_id}", get(task))
        .route(
            "/v1/github/apps/{app_ref}/deliveries/{delivery_id}",
            get(lookup),
        )
}

#[utoipa::path(get, path="/v1/github/capabilities", operation_id="getGitHubCapabilities", tag="github", security(("creatorBearer"=[])), responses((status=200,body=super::GitHubCapabilities),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn capabilities(
    State(state): State<ManagementState>,
    _creator: IntegrationOwner,
) -> Response {
    crate::http::problems::no_store(
        Json(super::GitHubCapabilities {
            store_ready: state.github.get().is_some(),
            dynamic_app_registration: true,
            native_app_webhook: true,
            webhook_secret_rotation: true,
            app_identity_verification: true,
            ownership_verification: false,
            automatic_reconciliation: false,
        })
        .into_response(),
    )
}

#[utoipa::path(post, path="/v1/github/apps", operation_id="createGitHubAppIntegration", tag="github", security(("creatorBearer"=[])), params(("Idempotency-Key"=String,Header)), request_body=super::CreateAppRequest, responses((status=201,body=super::AppIntegration),(status=200,body=super::AppIntegration),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn create_app(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    headers: HeaderMap,
    body: Result<Json<super::CreateAppRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Error> {
    let Json(body) = body.map_err(|_| Error::Invalid)?;
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    let key = header(&headers, "idempotency-key")?;
    let (integration, created) = gateway
        .create_app(&creator.subject, key, state.advertised_api_url(), body)
        .await?;
    Ok(crate::http::problems::no_store(
        (
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            Json(integration),
        )
            .into_response(),
    ))
}

#[utoipa::path(get, path="/v1/github/apps/{app_ref}", operation_id="getGitHubAppIntegration", tag="github", security(("creatorBearer"=[])), params(("app_ref"=String,Path)), responses((status=200,body=super::AppIntegration),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn read_app(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path(app_ref): Path<String>,
) -> Result<Response, Error> {
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    Ok(crate::http::problems::no_store(
        Json(gateway.read_app(&creator.subject, &app_ref).await?).into_response(),
    ))
}

#[utoipa::path(patch, path="/v1/github/apps/{app_ref}", operation_id="updateGitHubAppIntegration", tag="github", security(("creatorBearer"=[])), params(("app_ref"=String,Path),("Idempotency-Key"=String,Header)), request_body=super::UpdateAppRequest, responses((status=200,body=super::AppIntegration),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn update_app(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path(app_ref): Path<String>,
    headers: HeaderMap,
    body: Result<Json<super::UpdateAppRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Error> {
    let Json(body) = body.map_err(|_| Error::Invalid)?;
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    let app = gateway
        .mutate_app(
            &creator.subject,
            &app_ref,
            header(&headers, "idempotency-key")?,
            super::mutations::AppMutation::Metadata(body),
        )
        .await?;
    Ok(crate::http::problems::no_store(Json(app).into_response()))
}

#[utoipa::path(post, path="/v1/github/apps/{app_ref}/verification", operation_id="verifyGitHubAppIntegration", tag="github", security(("creatorBearer"=[])), params(("app_ref"=String,Path),("Idempotency-Key"=String,Header)), request_body=super::VerifyAppRequest, responses((status=200,body=super::AppIntegration),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn verify_app(
    State(state): State<ManagementState>,
    administrator: crate::auth::Administrator,
    Path(app_ref): Path<String>,
    headers: HeaderMap,
    body: Result<Json<super::VerifyAppRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Error> {
    let Json(body) = body.map_err(|_| Error::Invalid)?;
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    let app = gateway
        .mutate_app(
            &administrator.creator.subject,
            &app_ref,
            header(&headers, "idempotency-key")?,
            super::mutations::AppMutation::Verification {
                request: body,
                verifier: administrator.actor,
            },
        )
        .await?;
    Ok(crate::http::problems::no_store(Json(app).into_response()))
}

#[utoipa::path(put, path="/v1/github/apps/{app_ref}/webhook-secret", operation_id="setGitHubWebhookSecret", tag="github", security(("creatorBearer"=[])), params(("app_ref"=String,Path),("Idempotency-Key"=String,Header)), request_body=super::WebhookSecretRequest, responses((status=200,body=super::AppIntegration),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn webhook_secret(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path(app_ref): Path<String>,
    headers: HeaderMap,
    body: Result<Json<super::WebhookSecretRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Error> {
    let Json(body) = body.map_err(|_| Error::Invalid)?;
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    let app = gateway
        .mutate_app(
            &creator.subject,
            &app_ref,
            header(&headers, "idempotency-key")?,
            super::mutations::AppMutation::WebhookSecret(body),
        )
        .await?;
    Ok(crate::http::problems::no_store(Json(app).into_response()))
}

#[utoipa::path(get, path="/v1/github/apps", operation_id="listGitHubAppIntegrations", tag="github", security(("creatorBearer"=[])), params(super::ListAppsQuery), responses((status=200,body=super::AppList),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn list_apps(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    query: Result<Query<super::ListAppsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, Error> {
    let Query(query) = query.map_err(|_| Error::Invalid)?;
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    Ok(crate::http::problems::no_store(
        Json(gateway.list_apps(&creator.subject, query).await?).into_response(),
    ))
}

#[utoipa::path(post, path="/v1/github/installation-bindings", operation_id="bindGitHubInstallation", tag="github", security(("creatorBearer"=[])), request_body=BindRequest, responses((status=201,body=super::Binding),(status=200,body=super::Binding),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn bind(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    body: Result<Json<BindRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, Error> {
    let Json(body) = body.map_err(|_| Error::Invalid)?;
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    let (binding, created) = gateway.bind(&creator.subject, body).await?;
    Ok(crate::http::problems::no_store(
        (
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            Json(binding),
        )
            .into_response(),
    ))
}

#[utoipa::path(get, path="/v1/github/installation-bindings/{binding_id}", operation_id="getGitHubInstallationBinding", tag="github", security(("creatorBearer"=[])), params(("binding_id"=String,Path)), responses((status=200,body=super::Binding),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn binding(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    Ok(Json(gateway.binding(&creator.subject, &id).await?).into_response())
}

#[utoipa::path(post, path="/v1/github/apps/{app_ref}/webhooks", operation_id="receiveGitHubWebhook", tag="github", params(("app_ref"=String,Path),("X-GitHub-Delivery"=String,Header),("X-GitHub-Event"=String,Header),("X-Hub-Signature-256"=String,Header)), request_body=serde_json::Value, responses((status=202,body=super::Receipt),(status=200,body=super::Receipt),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn webhook(
    State(state): State<ManagementState>,
    Path(app_ref): Path<String>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<Response, Error> {
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    let delivery = header(&headers, "x-github-delivery")?;
    let content_type = header(&headers, "content-type").map_err(|_| Error::MediaType)?;
    let mut media = content_type.split(';').map(str::trim);
    if !media
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
    {
        return Err(Error::MediaType);
    }
    if let Some(parameter) = media.next() {
        let (name, value) = parameter.split_once('=').ok_or(Error::MediaType)?;
        if !name.trim().eq_ignore_ascii_case("charset")
            || !value.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
            || media.next().is_some()
        {
            return Err(Error::MediaType);
        }
    }
    if headers.contains_key("content-encoding")
        && !header(&headers, "content-encoding")
            .map_err(|_| Error::MediaType)?
            .eq_ignore_ascii_case("identity")
    {
        return Err(Error::MediaType);
    }
    let event = header(&headers, "x-github-event")?;
    if !super::config::valid_ref(delivery) || !super::config::valid_ref(event) {
        return Err(Error::Invalid);
    }
    let signature = header(&headers, "x-hub-signature-256")?;
    let (receipt, created) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let body = axum::body::to_bytes(request.into_body(), 256 * 1024)
            .await
            .map_err(|_| Error::TooLarge)?;
        gateway
            .receive(&app_ref, delivery, event, signature, &body)
            .await
    })
    .await
    .map_err(|_| Error::Unavailable)??;
    Ok(crate::http::problems::no_store(
        (
            if created && receipt.status == "accepted" {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            },
            Json(receipt),
        )
            .into_response(),
    ))
}

#[utoipa::path(get, path="/v1/github/deliveries/{receipt_id}", operation_id="getGitHubDelivery", tag="github", security(("creatorBearer"=[])), params(("receipt_id"=String,Path)), responses((status=200,body=super::Receipt),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn receipt(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    Ok(crate::http::problems::no_store(
        Json(gateway.receipt(&creator.subject, &id).await?).into_response(),
    ))
}

#[utoipa::path(get, path="/v1/github/tasks/{task_id}", operation_id="getGitHubTask", tag="github", security(("creatorBearer"=[])), params(("task_id"=String,Path)), responses((status=200,body=super::Task),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn task(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path(task_id): Path<String>,
) -> Result<Response, Error> {
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    Ok(Json(gateway.task(&creator.subject, &task_id).await?).into_response())
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Error> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(Error::Invalid)?;
    if values.next().is_some() {
        return Err(Error::Invalid);
    }
    Ok(value)
}

#[utoipa::path(get, path="/v1/github/apps/{app_ref}/deliveries/{delivery_id}", operation_id="lookupGitHubDelivery", tag="github", security(("creatorBearer"=[])), params(("app_ref"=String,Path),("delivery_id"=String,Path)), responses((status=200,body=super::Receipt),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn lookup(
    State(state): State<ManagementState>,
    creator: IntegrationOwner,
    Path((app_ref, delivery_id)): Path<(String, String)>,
) -> Result<Response, Error> {
    let gateway = state.github.get().ok_or(Error::Unavailable)?;
    if !super::config::valid_ref(&app_ref) || !super::config::valid_ref(&delivery_id) {
        return Err(Error::NotFound);
    }
    let id = super::identity("delivery", &(app_ref, delivery_id));
    Ok(crate::http::problems::no_store(
        Json(gateway.receipt(&creator.subject, &id).await?).into_response(),
    ))
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        use cmaeg_core::problem::ProblemCode;
        let (status, code) = match self {
            Self::Invalid => (StatusCode::BAD_REQUEST, ProblemCode::InvalidRequest),
            Self::TooLarge => (StatusCode::PAYLOAD_TOO_LARGE, ProblemCode::PayloadTooLarge),
            Self::MediaType => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ProblemCode::UnsupportedMediaType,
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                ProblemCode::GithubInstallationForbidden,
            ),
            Self::Signature => (
                StatusCode::UNAUTHORIZED,
                ProblemCode::GithubSignatureInvalid,
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, ProblemCode::GithubResourceNotFound),
            Self::Conflict => (StatusCode::CONFLICT, ProblemCode::GithubDeliveryConflict),
            Self::ManagementConflict | Self::VersionConflict(_) => {
                (StatusCode::CONFLICT, ProblemCode::Conflict)
            }
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ProblemCode::GithubSourceUnavailable,
            ),
        };
        crate::http::problems::no_store(crate::http::problems::problem(
            status,
            code,
            self.to_string(),
        ))
    }
}
