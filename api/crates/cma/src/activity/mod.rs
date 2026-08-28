//! Recording every request the control plane admits for service.
//!
//! Sits inside the middleware that establishes who is calling and outside
//! every handler, which is the only place that can see both the caller and how
//! their request came out. What it produces is the core's vocabulary and
//! nothing else — the translation lives in [`classify()`], and Layer 2 never
//! learns what carried the request.
//!
//! Two decisions worth knowing about.
//!
//! The write is awaited rather than spawned. It costs a round trip on each
//! request, and it is the difference between a record and a best effort: an
//! account of what happened that quietly loses entries under load is worth
//! very little precisely when it is worth most. A write that fails is logged
//! and never fails the caller's work.
//!
//! Rate-limit refusals happen outside this recorder. They are counted and
//! sampled into structured logs by the limiter instead: making refused traffic
//! buffer a body and await the MongoDB write it is protecting would let a flood
//! move the expensive work ahead of the boundary.
//!
//! The body is buffered only for the requests that carry a parameter worth
//! keeping, and only up to the same ceiling a handler would have accepted, so
//! this refuses exactly what the handler would have refused rather than
//! introducing a new limit of its own.

pub mod classify;

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer1_public::{
    rest::HttpProblem, ActedUpon, AuthenticatedSubject, SettingsTouched, TriggerSupportObserved,
};
use layer2_core::{
    ActivityDetail, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, DetailKey,
    SubjectId,
};

use crate::activity::classify::{classify, detail_from_body, detail_from_query, Classified};

/// Where requests are recorded.
pub struct ActivityRecorder {
    activity: Arc<ActivityService>,
}

impl ActivityRecorder {
    /// Build a recorder over the core's record of actions.
    pub fn new(activity: Arc<ActivityService>) -> Arc<Self> {
        Arc::new(Self { activity })
    }
}

/// Middleware recording what was asked for and how it came out.
pub async fn record(
    State(recorder): State<Arc<ActivityRecorder>>,
    req: Request,
    next: Next,
) -> Response {
    let Some(mut classified) = classify(req.method(), req.uri().path()) else {
        // Not something this service serves, or something it serves to nobody
        // in particular. Passed through untouched, body included.
        return next.run(req).await;
    };
    let query = req.uri().query().map(str::to_string);

    let (parts, body) = req.into_parts();
    // Placed by the middleware that establishes identity, which runs outside
    // this one. Absent means the request carried none, which is a fact worth
    // recording rather than a reason not to.
    let actor = parts
        .extensions
        .get::<AuthenticatedSubject>()
        .map(|s| s.0.clone());
    let request_correlation_id = classified
        .action
        .is_trigger()
        .then(|| layer1_public::rest::request_id_from_headers(&parts.headers))
        .flatten()
        .map(str::to_string);

    let (req, mut detail) = if classified.reads_body {
        match axum::body::to_bytes(body, crate::REQUEST_BODY_LIMIT_BYTES).await {
            Ok(bytes) => {
                let detail = detail_from_body(classified.action, &bytes);
                (Request::from_parts(parts, Body::from(bytes)), detail)
            }
            Err(e) => {
                // The body is gone, so the request cannot be served either.
                // Refusing it here with the entry that says so is the only
                // honest ending; passing on an empty body would fail the
                // request with a reason that has nothing to do with the cause.
                tracing::warn!(error = %e, "a request body was too large to serve");
                let refusal = too_large();
                store(
                    &recorder,
                    &classified,
                    actor,
                    ActivityDetail::new(),
                    ActivityOutcome::Rejected,
                )
                .await;
                return refusal;
            }
        }
    } else {
        (Request::from_parts(parts, body), ActivityDetail::new())
    };
    detail_from_query(query.as_deref(), &mut detail);
    if let Some(correlation_id) = request_correlation_id {
        detail.record(DetailKey::CorrelationId, correlation_id);
    }

    let response = next.run(req).await;
    let outcome = outcome_of(response.status());
    // Whose work it reached, when a handler established that it was not the
    // caller's own. Nothing here could derive it: the operator controls and
    // the owner's own controls are the same operation on the same shape of
    // address, so an entry for one would otherwise be indistinguishable from
    // an entry for the other — and which of the two happened is the whole
    // reason somebody reads this back.
    if let Some(ActedUpon(subject)) = response.extensions().get::<ActedUpon>() {
        detail.record(DetailKey::Subject, subject.as_str());
    }
    if let Some(touched) = response.extensions().get::<SettingsTouched>() {
        if !touched.changed.is_empty() {
            detail.record(DetailKey::Setting, touched.changed.join(","));
        }
        if let Some(card) = &touched.rate_card {
            detail.record(DetailKey::RateCard, card.clone());
        }
    }
    if let Some(observed) = response.extensions().get::<TriggerSupportObserved>() {
        classified.target = Some(ActivityTarget::Launch(observed.launch_id.clone()));
        if let Some(correlation_id) = observed.correlation_id.as_ref() {
            detail.record(DetailKey::CorrelationId, correlation_id);
        }
    }
    store(&recorder, &classified, actor, detail, outcome).await;
    response
}

/// Hand one entry to the core.
async fn store(
    recorder: &ActivityRecorder,
    classified: &Classified,
    actor: Option<SubjectId>,
    detail: ActivityDetail,
    outcome: ActivityOutcome,
) {
    let mut entry = ActivityRecord::new(classified.action, outcome);
    entry.actor = actor;
    entry.target = classified.target.clone();
    entry.detail = detail;
    crate::trigger::observability::record_activity(&recorder.activity, entry).await;
}

/// How a response's status reads as an outcome.
///
/// The grouping is what an operator actually asks of the record: was the
/// caller turned away, did they send something unusable, or did the service
/// fail them. A refusal to say whether something exists counts as being turned
/// away, because that is what it is.
fn outcome_of(status: StatusCode) -> ActivityOutcome {
    match status {
        s if s.is_success() || s.is_redirection() => ActivityOutcome::Succeeded,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            ActivityOutcome::Denied
        }
        s if s.is_client_error() => ActivityOutcome::Rejected,
        _ => ActivityOutcome::Failed,
    }
}

/// The refusal for a body too large to serve.
fn too_large() -> Response {
    HttpProblem::RequestTooLarge.into_response()
}

#[cfg(test)]
#[path = "middleware_tests.rs"]
mod middleware_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_served_request_reads_as_success() {
        for status in [StatusCode::OK, StatusCode::CREATED, StatusCode::NO_CONTENT] {
            assert_eq!(outcome_of(status), ActivityOutcome::Succeeded, "{status}");
        }
        // The sign-in flow answers with a redirect, and it succeeded.
        assert_eq!(
            outcome_of(StatusCode::SEE_OTHER),
            ActivityOutcome::Succeeded
        );
    }

    #[test]
    fn a_caller_turned_away_reads_as_denied() {
        // Including "not found", which is how this service answers a caller
        // who may not see something — the record should say they were turned
        // away, because they were.
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            assert_eq!(outcome_of(status), ActivityOutcome::Denied, "{status}");
        }
    }

    #[test]
    fn an_unusable_request_reads_as_rejected() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::CONFLICT,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::PAYLOAD_TOO_LARGE,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            assert_eq!(outcome_of(status), ActivityOutcome::Rejected, "{status}");
        }
    }

    #[test]
    fn a_failure_of_ours_reads_as_failed() {
        // Distinct from a rejection: nothing the caller did caused it, and an
        // operator scanning for problems wants exactly these.
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(outcome_of(status), ActivityOutcome::Failed, "{status}");
        }
    }

    #[test]
    fn a_body_too_large_is_refused_with_a_problem_document() {
        let response = too_large();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
