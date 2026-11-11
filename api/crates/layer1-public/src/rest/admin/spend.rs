//! Operator view of the platform spend period and its cloud observations.
//! Values are private accounting units and never leave this surface.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use axum::{
    extract::{rejection::JsonRejection, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use layer2_core::budget::{
    CloudObservation, CloudObservationInput, PlatformPeriodLedger, PlatformSpendState, SpendLevels,
};
use layer2_core::CoreError;
use serde::{Deserialize, Serialize};

use super::{AdminState, NotAnOperator};
use crate::rest::{problem_document, problem_response};
use crate::AuthenticatedCaller;

const MAXIMUM_UNITS: u64 = (1 << 53) - 1;

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/spend", get(read))
        .route("/api/v1/admin/spend/observations", post(observe))
}

async fn read(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SpendView>, SpendFailure> {
    let service = state.spend.as_ref().ok_or(SpendFailure::Unavailable)?;
    Ok(Json(service.read(&caller).await?.into()))
}

async fn observe(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ObservationRequest>, JsonRejection>,
) -> Result<Json<SpendView>, SpendFailure> {
    caller.require(layer2_core::Capability::ConfigurePlatform)?;
    let service = state.spend.as_ref().ok_or(SpendFailure::Unavailable)?;
    let Json(body) = body.map_err(|_| SpendFailure::Invalid(None))?;
    let observed_through = layer2_core::seconds_from_rfc3339(&body.observed_through)
        .ok_or(SpendFailure::Invalid(None))?;
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
        .ok_or(SpendFailure::Unavailable)?;
    if body.units > MAXIMUM_UNITS || observed_through > now {
        return Err(SpendFailure::Invalid(None));
    }
    let input = CloudObservationInput {
        observed_through,
        units: body.units,
    };
    Ok(Json(
        service
            .record_cloud_observation(&caller, input)
            .await?
            .into(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationRequest {
    /// Only `cloud` in this delivery; model-provider and adjustment sources are deferred.
    #[allow(dead_code)]
    source: ObservationSource,
    observed_through: String,
    units: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ObservationSource {
    Cloud,
}

#[derive(Serialize)]
struct SpendView {
    settings_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    levels: Option<LevelsView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ledger: Option<LedgerView>,
}

#[derive(Serialize)]
struct LevelsView {
    warning: u64,
    stop: u64,
    disposable: u64,
}

#[derive(Serialize)]
struct LedgerView {
    period_start: String,
    period_end: String,
    settings_revision: u64,
    spent: u64,
    reserved: u64,
    adjustments: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud: Option<CloudView>,
    closures: Vec<ClosureView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evaluated_at: Option<String>,
}

#[derive(Serialize)]
struct CloudView {
    observed_through: String,
    units: u64,
    recorded_at: String,
    settings_revision: u64,
}

#[derive(Serialize)]
struct ClosureView {
    reason: layer2_core::budget::ClosureReason,
    since: String,
    settings_revision: u64,
    committed_at_entry: u64,
}

fn at(seconds: u64) -> String {
    layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds))
}

impl From<PlatformSpendState> for SpendView {
    fn from(value: PlatformSpendState) -> Self {
        Self {
            settings_revision: value.settings_revision,
            levels: value.levels.map(
                |SpendLevels {
                     warning,
                     stop,
                     disposable,
                 }| LevelsView {
                    warning,
                    stop,
                    disposable,
                },
            ),
            committed: value.committed,
            ledger: value.ledger.map(LedgerView::from),
        }
    }
}

impl From<PlatformPeriodLedger> for LedgerView {
    fn from(value: PlatformPeriodLedger) -> Self {
        Self {
            period_start: at(value.period_start),
            period_end: at(value.period_end),
            settings_revision: value.settings_revision,
            spent: value.spent,
            reserved: value.reserved,
            adjustments: value.adjustments,
            cloud: value.cloud.map(|cloud: CloudObservation| CloudView {
                observed_through: at(cloud.observed_through),
                units: cloud.units,
                recorded_at: at(cloud.recorded_at),
                settings_revision: cloud.settings_revision,
            }),
            closures: value
                .closures
                .into_iter()
                .map(|closure| ClosureView {
                    reason: closure.reason,
                    since: at(closure.since),
                    settings_revision: closure.settings_revision,
                    committed_at_entry: closure.committed_at_entry,
                })
                .collect(),
            evaluated_at: (value.evaluated_at > 0).then(|| at(value.evaluated_at)),
        }
    }
}

enum SpendFailure {
    NotAnOperator,
    Invalid(Option<String>),
    WritesUnavailable,
    Unavailable,
}

impl From<CoreError> for SpendFailure {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Forbidden => Self::NotAnOperator,
            CoreError::ModelBudgetWritesUnavailable => Self::WritesUnavailable,
            CoreError::InvalidRequest(detail) => Self::Invalid(Some(detail)),
            _ => Self::Unavailable,
        }
    }
}

impl IntoResponse for SpendFailure {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self {
            Self::NotAnOperator => return NotAnOperator.into_response(),
            Self::Invalid(detail) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-request",
                detail.unwrap_or_else(|| {
                    "The observation does not match the platform spend contract.".into()
                }),
                "invalid_request",
            ),
            Self::WritesUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "model-budget-writes-unavailable",
                "Saving spend controls is not available yet.".into(),
                "model_budget_writes_unavailable",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "platform-spend-unavailable",
                "Platform spend accounting could not be read or changed.".into(),
                "platform_spend_unavailable",
            ),
        };
        problem_response(status, problem_document(status, slug, &detail, code))
    }
}
