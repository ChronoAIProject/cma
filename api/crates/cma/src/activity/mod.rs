//! Request-level activity recording at the composed HTTP boundary.

use std::sync::Arc;

use crate::agent_manager::relay::{ChargedQueuedCancellation, DeferQueuedCancellationActivity};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use layer1_public::{responses::v2::QueuedCancellationNeedsOwner, AuthenticatedSubject};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, SubjectId,
};

mod policy;
pub(crate) mod queue;

pub struct ActivityRecorder {
    activity: Arc<ActivityService>,
    sink: Option<Arc<dyn queue::ActivitySink>>,
    policy: policy::RequestAuditPolicy,
}

impl ActivityRecorder {
    #[cfg(test)]
    pub fn new(activity: Arc<ActivityService>) -> Arc<Self> {
        Self::configured(activity, None, false)
    }

    #[cfg(test)]
    pub(crate) fn with_sink(
        activity: Arc<ActivityService>,
        sink: Arc<dyn queue::ActivitySink>,
    ) -> Arc<Self> {
        Self::configured(activity, Some(sink), false)
    }
    pub(crate) fn configured(
        activity: Arc<ActivityService>,
        sink: Option<Arc<dyn queue::ActivitySink>>,
        sampling: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity,
            sink,
            policy: policy::RequestAuditPolicy::new(sampling),
        })
    }
}

/// A routed cancellation can finish on another manager. Keep the original
/// request's audit metadata with the ingress manager until a final status exists.
#[derive(Clone)]
pub(crate) struct PendingActivity {
    recorder: Arc<ActivityRecorder>,
    method: Method,
    matched_route: Option<String>,
    path: String,
    actor: Option<SubjectId>,
}

impl PendingActivity {
    fn is_usage_view_probe(&self, response: &Response) -> bool {
        response.status() == StatusCode::NOT_IMPLEMENTED
            && response
                .extensions()
                .get::<layer1_public::rest::UsageReportViewDisabled>()
                .is_some()
            && matches!(
                (&self.method, self.matched_route.as_deref()),
                (
                    &Method::GET,
                    Some("/api/v1/admin/usage/metrics/view" | "/api/v1/usage/metrics/view")
                ) | (
                    &Method::POST,
                    Some(
                        "/api/v1/admin/usage/metrics/initial/view"
                            | "/api/v1/usage/metrics/initial/view"
                    )
                )
            )
    }

    pub(crate) async fn record(self, status: StatusCode) {
        let action = classify(&self.method, &self.path);
        let mut activity =
            ActivityRecord::new(action, outcome(status)).on(ActivityTarget::Resource(self.path));
        activity.actor = self.actor;
        if !self.recorder.policy.retain(
            &self.method,
            self.matched_route.as_deref(),
            status,
            &activity,
        ) {
            return;
        }
        if let Some(sink) = &self.recorder.sink {
            sink.try_enqueue(activity);
            return;
        }
        if let Err(error) = self.recorder.activity.record(&activity).await {
            tracing::warn!(error = %error, "could not persist request activity");
        }
    }
}

/// Record admitted requests without retaining headers, credentials, or bodies.
pub async fn record(
    State(recorder): State<Arc<ActivityRecorder>>,
    request: Request,
    next: Next,
) -> Response {
    if request
        .extensions()
        .get::<ChargedQueuedCancellation>()
        .is_some()
    {
        return next.run(request).await;
    }
    let defer = request
        .extensions()
        .get::<DeferQueuedCancellationActivity>()
        .is_some();
    let method = request.method().clone();
    let matched_route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned());
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| request.uri().path())
        .to_owned();
    if matches!(path.as_str(), "/livez" | "/ready")
        || path == layer1_public::rest::creation_outcomes::LOOKUP_PATH
    {
        return next.run(request).await;
    }
    let actor = request
        .extensions()
        .get::<AuthenticatedSubject>()
        .map(|subject| subject.0.clone());
    let pending = PendingActivity {
        recorder,
        method,
        matched_route,
        path,
        actor,
    };
    let mut response = next.run(request).await;
    if pending.is_usage_view_probe(&response) {
        return response;
    }
    if defer
        && response
            .extensions()
            .get::<QueuedCancellationNeedsOwner>()
            .is_some()
    {
        response.extensions_mut().insert(pending);
    } else {
        pending.record(response.status()).await;
    }
    response
}

fn classify(method: &Method, path: &str) -> ActivityAction {
    if path.starts_with("/auth/") && path != "/auth/session" {
        return ActivityAction::Authenticate;
    }
    if method == Method::POST
        && matches!(
            path,
            "/api/v1/profile-authoring/repositories/resolve"
                | "/api/v1/admin/usage/metrics/options"
        )
    {
        return ActivityAction::Read;
    }
    match *method {
        Method::GET | Method::HEAD => ActivityAction::Read,
        Method::POST => ActivityAction::Create,
        Method::PUT | Method::PATCH => ActivityAction::Update,
        Method::DELETE => ActivityAction::Delete,
        _ => ActivityAction::Read,
    }
}

fn outcome(status: StatusCode) -> ActivityOutcome {
    if status.is_success() || status.is_redirection() {
        ActivityOutcome::Succeeded
    } else if status.is_client_error() {
        ActivityOutcome::Refused(status.as_u16().to_string())
    } else {
        ActivityOutcome::Failed(status.as_u16().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_http_classification_has_no_resource_vocabulary() {
        assert_eq!(classify(&Method::GET, "/resource"), ActivityAction::Read);
        assert_eq!(classify(&Method::POST, "/resource"), ActivityAction::Create);
        assert_eq!(
            classify(&Method::GET, "/auth/logout"),
            ActivityAction::Authenticate
        );
    }

    #[test]
    fn repository_verification_is_recorded_as_a_read() {
        assert_eq!(
            classify(
                &Method::POST,
                "/api/v1/profile-authoring/repositories/resolve"
            ),
            ActivityAction::Read
        );
        assert_eq!(
            classify(&Method::POST, "/api/v1/admin/usage/metrics/options"),
            ActivityAction::Read
        );
    }

    #[test]
    fn outcomes_keep_stable_status_evidence() {
        assert_eq!(outcome(StatusCode::NO_CONTENT), ActivityOutcome::Succeeded);
        assert_eq!(
            outcome(StatusCode::FORBIDDEN),
            ActivityOutcome::Refused("403".into())
        );
        assert_eq!(
            outcome(StatusCode::BAD_GATEWAY),
            ActivityOutcome::Failed("502".into())
        );
    }
}

#[cfg(test)]
mod audit_contract_tests;
#[cfg(test)]
mod http_tests;
