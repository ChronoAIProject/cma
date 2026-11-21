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
use utoipa::{OpenApi, ToSchema};

use super::{AdminState, NotAnOperator};
use crate::rest::{problem_document, problem_response};
use crate::AuthenticatedCaller;

const MAXIMUM_UNITS: u64 = (1 << 53) - 1;

#[cfg(test)]
#[path = "spend_contract_tests.rs"]
mod contract_tests;

pub(super) fn routes() -> Router<Arc<AdminState>> {
    Router::new()
        .route("/api/v1/admin/spend", get(read))
        .route("/api/v1/admin/spend/observations", post(observe))
}

/// Read platform spend with ConfigurePlatform authority. Disabled metering is distinct
/// from unavailable accounting. An absent service returns 503 before capability checks.
#[utoipa::path(get, path = "/api/v1/admin/spend", tag = "administration",
    responses(
        (status = 200, description = "Current spend state, including explicitly disabled metering", body = SpendView),
        (status = 401, description = "not_authenticated", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "forbidden: not-an-operator; ConfigurePlatform is required when the service is present", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "platform_spend_unavailable: service absent or accounting could not be read", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn read(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SpendView>, SpendFailure> {
    let service = state.spend.as_ref().ok_or(SpendFailure::Unavailable)?;
    Ok(Json(service.read(&caller).await?.into()))
}

/// Record cumulative cloud units for the current period with ConfigurePlatform authority.
/// Writes require both CHRONO_MODEL_BUDGET_WRITES_ENABLED and
/// CHRONO_PLATFORM_SPEND_WRITES_ENABLED (platform_spend_writes_available in settings).
/// An identical observation at the same instant is idempotent; a different value
/// conflicts with 422. Authorization and
/// service availability precede body validation, which precedes the write gate.
#[utoipa::path(post, path = "/api/v1/admin/spend/observations", tag = "administration",
    request_body = ObservationRequest,
    responses(
        (status = 200, description = "Spend state after recording or repeating an identical observation", body = SpendView),
        (status = 401, description = "not_authenticated", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "forbidden: not-an-operator; ConfigurePlatform is required", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "invalid_request: malformed JSON, unknown or missing fields, unsupported source, invalid or future timestamp, unsafe units, observation outside the current period, or a conflicting observation at the same instant", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "model_budget_writes_unavailable: observation writes disabled; platform_spend_unavailable: service absent or accounting could not be read or changed", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
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

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ObservationRequest {
    /// Only `cloud` in this delivery; model-provider and adjustment sources are deferred.
    #[allow(dead_code)]
    source: ObservationSource,
    /// RFC 3339 instant within the current platform period and no later than server time.
    #[schema(format = DateTime)]
    observed_through: String,
    /// Cumulative private accounting units, not a currency amount.
    #[schema(minimum = 0, maximum = 9007199254740991_u64)]
    units: u64,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum ObservationSource {
    Cloud,
}

#[derive(Serialize, ToSchema)]
struct SpendView {
    /// False when budgets or the platform cloud source are not configured:
    /// metering is off, which is distinct from enabled but unavailable.
    metering_enabled: bool,
    settings_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    levels: Option<LevelsView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ledger: Option<LedgerView>,
}

#[derive(Serialize, ToSchema)]
struct LevelsView {
    warning: u64,
    stop: u64,
    disposable: u64,
}

#[derive(Serialize, ToSchema)]
struct LedgerView {
    #[schema(format = DateTime)]
    period_start: String,
    #[schema(format = DateTime)]
    period_end: String,
    settings_revision: u64,
    spent: u64,
    reserved: u64,
    adjustments: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud: Option<CloudView>,
    closures: Vec<ClosureView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(format = DateTime)]
    evaluated_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct CloudView {
    #[schema(format = DateTime)]
    observed_through: String,
    units: u64,
    #[schema(format = DateTime)]
    recorded_at: String,
    settings_revision: u64,
}

#[derive(Serialize, ToSchema)]
struct ClosureView {
    #[schema(schema_with = closure_reason_schema)]
    reason: layer2_core::budget::ClosureReason,
    #[schema(format = DateTime)]
    since: String,
    settings_revision: u64,
    committed_at_entry: u64,
}

fn closure_reason_schema() -> utoipa::openapi::schema::Object {
    use layer2_core::budget::ClosureReason;
    use utoipa::openapi::schema::{ObjectBuilder, Type};
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(
            [
                ClosureReason::SpendWarning,
                ClosureReason::SpendStop,
                ClosureReason::MeteringUnavailable,
            ]
            .map(|reason| serde_json::to_value(reason).expect("closure reason serializes")),
        ))
        .build()
}

#[derive(OpenApi)]
#[openapi(
    paths(read, observe),
    components(schemas(
        ObservationRequest,
        ObservationSource,
        SpendView,
        LevelsView,
        LedgerView,
        CloudView,
        ClosureView
    ))
)]
struct SpendApi;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    SpendApi::openapi()
}

fn at(seconds: u64) -> String {
    layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds))
}

impl From<PlatformSpendState> for SpendView {
    fn from(value: PlatformSpendState) -> Self {
        Self {
            metering_enabled: value.metering_enabled,
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
