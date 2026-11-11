//! Aggregate-neutral records written after a successful sign-in.

use std::sync::Arc;

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use layer2_core::budget::{PlatformAdmissionState, PlatformSpendLedger};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget,
    PublicIdentity, StoreError, SubjectId, UserDirectory,
};

use super::{profile::Profile, AuthState};

pub struct SignInRecords {
    pub users: Arc<UserDirectory>,
    pub activity: Arc<ActivityService>,
}

impl AuthState {
    pub(super) async fn note_sign_in(&self, subject: &SubjectId, profile: &Profile) {
        let Some(records) = self.records.as_ref() else {
            return;
        };
        if let Err(error) = records
            .users
            .observe(&PublicIdentity {
                subject: subject.clone(),
                display_name: profile.display_name.clone(),
                avatar_url: profile.avatar_url.clone(),
            })
            .await
        {
            tracing::warn!(error = %error, "could not update the user directory after sign-in");
        }
        let activity =
            ActivityRecord::new(ActivityAction::Authenticate, ActivityOutcome::Succeeded)
                .by(subject.clone())
                .on(ActivityTarget::Subject(subject.clone()));
        if let Err(error) = records.activity.record(&activity).await {
            tracing::warn!(error = %error, "could not record successful sign-in");
        }
    }
}

/// Registration is the first durable owner record, written at the sign-in
/// callback; the platform admission state closes it for never-seen subjects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegistrationDecision {
    Admitted,
    Existing,
    Closed,
}

pub(super) fn registration_decision(
    state: &PlatformAdmissionState,
    existing: bool,
) -> RegistrationDecision {
    if !state.closed_for_registration() {
        RegistrationDecision::Admitted
    } else if existing {
        RegistrationDecision::Existing
    } else {
        RegistrationDecision::Closed
    }
}

impl AuthState {
    pub(crate) fn with_platform_spend(
        mut self: Arc<Self>,
        spend: Arc<dyn PlatformSpendLedger>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("platform spend is composed before sharing authentication")
            .spend = Some(spend);
        self
    }

    /// The authority, at the callback: reads fail closed. The directory is
    /// consulted only while a closure exists.
    pub(super) async fn admit_registration(
        &self,
        subject: &SubjectId,
    ) -> Result<RegistrationDecision, StoreError> {
        let Some(spend) = self.spend.as_ref() else {
            return Ok(RegistrationDecision::Admitted);
        };
        let state = spend
            .admission_state(now_seconds())
            .await
            .map_err(|error| StoreError::Unavailable(error.to_string()))?;
        if !state.closed_for_registration() {
            return Ok(RegistrationDecision::Admitted);
        }
        let existing = match self.records.as_ref() {
            Some(records) => records.users.user(subject).await?.is_some(),
            None => false,
        };
        Ok(registration_decision(&state, existing))
    }

    /// Advisory, at the sign-up entry: an unreadable state proceeds and the
    /// callback decides, because an identity can also be created upstream.
    pub(super) async fn registration_open(&self) -> bool {
        let Some(spend) = self.spend.as_ref() else {
            return true;
        };
        spend
            .admission_state(now_seconds())
            .await
            .map_or(true, |state| !state.closed_for_registration())
    }
}

fn now_seconds() -> u64 {
    layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap_or(u64::MAX)
}

/// Its own body: the sign-in failure body asks for a new attempt, which is
/// wrong while sign-ups are closed. No `login_url` and no `Retry-After`.
pub(super) fn registration_closed(clear_binding: Option<(&str, bool)>) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "title": "Sign-up is temporarily closed",
            "detail": "New sign-ups are temporarily closed. Existing accounts are unaffected.",
            "code": "registration_closed",
        })),
    )
        .into_response();
    if let Some((name, secure)) = clear_binding {
        response.headers_mut().append(
            header::SET_COOKIE,
            super::session_routes::issued_cookie(name, "", secure, 0),
        );
    }
    response
}

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
