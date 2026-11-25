//! Durable operator targets for image/resource based native warm pools.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, put},
    Json, Router,
};
use layer2_core::{Caller, Capability};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{AdminState, CollectionProblem};
use crate::AuthenticatedCaller;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureWarmPoolBody {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub target_replicas: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeWarmPoolBody {
    pub expected_revision: u64,
    pub target_replicas: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct WarmPoolView {
    pub id: String,
    pub revision: u64,
    pub image: String,
    /// Immutable image, resources, home and Pod policy identity.
    pub config_sha256: String,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub target_replicas: u32,
    pub ready_replicas: Option<u32>,
    pub observed: bool,
    pub status: String,
    pub resource_name: Option<String>,
    pub error: Option<String>,
    /// The deployment-seeded shared desktop pool, resized against `machine_image`.
    #[serde(default)]
    pub machine: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct WarmPoolList {
    pub items: Vec<WarmPoolView>,
    pub default_image: String,
    pub default_cpu_millis: u32,
    pub default_memory_mib: u32,
    /// Exact current default configuration; null when it cannot be planned.
    pub default_config_sha256: Option<String>,
    pub available: bool,
    /// Configured shared desktop image; null when this deployment seeds no desktop pool.
    #[serde(default)]
    pub machine_image: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmPoolError {
    Forbidden,
    Invalid,
    Conflict,
    NotFound,
    Unavailable,
    ProviderConfigurationDrift,
}

impl From<WarmPoolError> for CollectionProblem {
    fn from(error: WarmPoolError) -> Self {
        match error {
            WarmPoolError::Forbidden => Self::NotAnOperator,
            WarmPoolError::Invalid => Self::InvalidRequest,
            WarmPoolError::Conflict => Self::Conflict,
            WarmPoolError::NotFound => Self::NotFound,
            WarmPoolError::Unavailable | WarmPoolError::ProviderConfigurationDrift => {
                Self::Unavailable
            }
        }
    }
}

/// Implementations persist commands before touching provider resources.
#[async_trait]
pub trait WarmPoolAdministration: Send + Sync {
    async fn list(&self, caller: &Caller) -> Result<WarmPoolList, WarmPoolError>;
    async fn configure(
        &self,
        caller: &Caller,
        key: &str,
        body: ConfigureWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError>;
    async fn resize(
        &self,
        caller: &Caller,
        id: &str,
        body: ResizeWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError>;
}

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/warm-pools", get(list).post(configure))
        .route("/api/v1/admin/warm-pools/{id}", put(resize))
}

#[utoipa::path(get, path = "/api/v1/admin/warm-pools", tag = "administration",
    responses((status = 200, body = WarmPoolList), (status = 403, body = crate::rest::ProblemDocument)))]
pub(crate) async fn list(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<WarmPoolList>, CollectionProblem> {
    caller.require(Capability::ConfigurePlatform)?;
    let service = state
        .warm_pools
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?;
    Ok(Json(service.list(&caller).await?))
}

#[utoipa::path(post, path = "/api/v1/admin/warm-pools", tag = "administration",
    params(("Idempotency-Key" = String, Header)), request_body = ConfigureWarmPoolBody,
    responses((status = 202, body = WarmPoolView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn configure(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    headers: HeaderMap,
    body: Result<Json<ConfigureWarmPoolBody>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<WarmPoolView>), CollectionProblem> {
    caller.require(Capability::ConfigurePlatform)?;
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 128 && v.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or(CollectionProblem::InvalidRequest)?;
    let body = body.map_err(|_| CollectionProblem::InvalidRequest)?.0;
    let service = state
        .warm_pools
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(service.configure(&caller, key, body).await?),
    ))
}

#[utoipa::path(put, path = "/api/v1/admin/warm-pools/{id}", tag = "administration",
    params(("id" = String, Path)), request_body = ResizeWarmPoolBody,
    responses((status = 202, body = WarmPoolView), (status = 409, body = crate::rest::ProblemDocument)))]
pub(crate) async fn resize(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<ResizeWarmPoolBody>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<WarmPoolView>), CollectionProblem> {
    caller.require(Capability::ConfigurePlatform)?;
    let body = body.map_err(|_| CollectionProblem::InvalidRequest)?.0;
    let service = state
        .warm_pools
        .as_ref()
        .ok_or(CollectionProblem::Unavailable)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(service.resize(&caller, &id, body).await?),
    ))
}
