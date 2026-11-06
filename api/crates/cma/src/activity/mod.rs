//! Request-level activity recording at the composed HTTP boundary.

use std::sync::Arc;

use axum::{
    extract::{MatchedPath, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use layer1_public::AuthenticatedSubject;
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget,
};

pub struct ActivityRecorder {
    activity: Arc<ActivityService>,
}

impl ActivityRecorder {
    pub fn new(activity: Arc<ActivityService>) -> Arc<Self> {
        Arc::new(Self { activity })
    }
}

/// Record admitted requests without retaining headers, credentials, or bodies.
pub async fn record(
    State(recorder): State<Arc<ActivityRecorder>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| request.uri().path())
        .to_owned();
    if matches!(path.as_str(), "/livez" | "/ready") {
        return next.run(request).await;
    }
    let actor = request
        .extensions()
        .get::<AuthenticatedSubject>()
        .map(|subject| subject.0.clone());
    let action = classify(request.method(), &path);
    let response = next.run(request).await;
    let mut activity =
        ActivityRecord::new(action, outcome(response.status())).on(ActivityTarget::Resource(path));
    activity.actor = actor;
    if let Err(error) = recorder.activity.record(&activity).await {
        tracing::warn!(error = %error, "could not persist request activity");
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
