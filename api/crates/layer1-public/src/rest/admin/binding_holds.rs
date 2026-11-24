use std::{collections::BTreeSet, sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use layer2_core::binding_diagnostics::{
    AdapterFailureClass, BindingDiagnosticsError, BindingHold, BindingHoldOverview, HoldPhase,
    HoldReason,
};
use serde::Serialize;
use utoipa::ToSchema;

use super::AdminState;
use crate::AuthenticatedCaller;

#[derive(Serialize, ToSchema)]
pub struct BindingHoldView {
    sandbox_id: String,
    owner: String,
    owner_name: Option<String>,
    #[schema(value_type = String)]
    reason: HoldReason,
    #[schema(value_type = String)]
    phase: HoldPhase,
    first_held_at: String,
    attempts: u32,
    retry_not_before: Option<String>,
    #[schema(value_type = Option<String>)]
    adapter_error_class: Option<AdapterFailureClass>,
}

#[derive(Serialize, ToSchema)]
pub struct BindingHoldSampleView {
    observed_at: String,
    truncated: bool,
    holds: Vec<BindingHoldView>,
}

#[derive(Serialize, ToSchema)]
pub struct BindingHoldReportView {
    reporter: String,
    sample: BindingHoldSampleView,
}

/// Registered processes are an observation boundary, not a deployment census.
#[derive(Serialize, ToSchema)]
pub struct BindingHoldOverviewView {
    observed_at: String,
    observed_sandboxes: usize,
    registered_reporters: usize,
    unavailable_reporters: usize,
    truncated: bool,
    reports: Vec<BindingHoldReportView>,
}

#[utoipa::path(get, path = "/api/v1/admin/runtime-binding-holds", tag = "administration",
    responses((status = 200, body = BindingHoldOverviewView),
              (status = 403, body = crate::rest::ProblemDocument),
              (status = 404, description = "Diagnostics are disabled"),
              (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Response {
    let Some(service) = &state.binding_diagnostics else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let result = tokio::time::timeout(Duration::from_secs(7), async {
        let value = service.read(&caller).await?;
        let owners: Vec<_> = value
            .reports
            .iter()
            .flat_map(|report| report.sample.holds.iter().map(|hold| hold.owner.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(200)
            .collect();
        let names = tokio::time::timeout(
            Duration::from_secs(1),
            super::super::public_names::resolve(&state.users, &owners),
        )
        .await
        .unwrap_or_default();
        let bytes = serde_json::to_vec(&view(value, &names))
            .map_err(|_| BindingDiagnosticsError::Unavailable)?;
        if bytes.len() > 512 * 1024 {
            return Err(BindingDiagnosticsError::Unavailable);
        }
        Ok::<_, BindingDiagnosticsError>(bytes)
    })
    .await;
    let mut response = match result {
        Ok(Ok(bytes)) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Ok(Err(BindingDiagnosticsError::Forbidden)) => super::NotAnOperator.into_response(),
        Ok(Err(BindingDiagnosticsError::Unavailable)) | Err(_) => super::super::problem_response(
            StatusCode::SERVICE_UNAVAILABLE,
            super::super::problem_document(
                StatusCode::SERVICE_UNAVAILABLE,
                "binding-observation-unavailable",
                "Runtime binding observations could not be read.",
                "binding_observation_unavailable",
            ),
        ),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
}

fn view(
    value: BindingHoldOverview,
    names: &std::collections::BTreeMap<String, String>,
) -> BindingHoldOverviewView {
    BindingHoldOverviewView {
        observed_at: value.observed_at,
        observed_sandboxes: value.observed_sandboxes,
        registered_reporters: value.registered_reporters,
        unavailable_reporters: value.unavailable_reporters,
        truncated: value.truncated,
        reports: value
            .reports
            .into_iter()
            .map(|report| BindingHoldReportView {
                reporter: report.reporter,
                sample: BindingHoldSampleView {
                    observed_at: report.sample.observed_at,
                    truncated: report.sample.truncated,
                    holds: report
                        .sample
                        .holds
                        .into_iter()
                        .map(|hold| hold_view(hold, names))
                        .collect(),
                },
            })
            .collect(),
    }
}

fn hold_view(
    hold: BindingHold,
    names: &std::collections::BTreeMap<String, String>,
) -> BindingHoldView {
    BindingHoldView {
        owner_name: names
            .get(hold.owner.as_str())
            .filter(|name| name.len() <= 200 && !name.chars().any(char::is_control))
            .cloned(),
        sandbox_id: hold.sandbox_id.0,
        owner: hold.owner.0,
        reason: hold.reason,
        phase: hold.phase,
        first_held_at: hold.first_held_at,
        attempts: hold.attempts,
        retry_not_before: hold.retry_not_before,
        adapter_error_class: hold.adapter_error_class,
    }
}
