use layer2_core::{LaunchFailure, PullRequestRecord};

#[derive(Debug)]
pub(crate) enum PullRequestStatusDisposition {
    /// Persist the observation and schedule the next bounded read if the PR is
    /// still open.
    Observed {
        pull_request: Box<PullRequestRecord>,
        next_attempt_at: Option<String>,
        provider_request_id: Option<String>,
    },
    /// Retain a safe failure and retry only after the supplied deadline.
    Retry {
        failure: LaunchFailure,
        next_attempt_at: String,
        provider_request_id: Option<String>,
    },
    /// Retain a stale/manual-change failure while leaving the linked PR
    /// readable. No automatic retry is scheduled until an explicit refresh.
    Suspended {
        failure: LaunchFailure,
        provider_request_id: Option<String>,
    },
    /// Persist the read failure and apply the normal top-level Launch failure
    /// taxonomy, for example authentication recovery or terminal identity
    /// drift. The linked provider record remains available for support.
    FailLaunch {
        failure: LaunchFailure,
        provider_request_id: Option<String>,
    },
}
