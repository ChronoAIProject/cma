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
    if existing {
        RegistrationDecision::Existing
    } else if state.closed_for_registration() {
        RegistrationDecision::Closed
    } else {
        RegistrationDecision::Admitted
    }
}

impl AuthState {
    pub(crate) fn with_platform_spend(
        self: Arc<Self>,
        spend: Arc<dyn PlatformSpendLedger>,
    ) -> Arc<Self> {
        self.with_registration_policy(Arc::new(super::registration::SpendRegistrationPolicy(
            spend,
        )))
    }

    pub(crate) fn with_registration_policy(
        mut self: Arc<Self>,
        spend: Arc<dyn super::registration::RegistrationPolicy>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("platform spend is composed before sharing authentication")
            .spend = Some(spend);
        self
    }

    /// The authority, before sign-in persistence: existing identity and owned
    /// resource evidence survives a failed best-effort directory observation.
    pub(super) async fn admit_registration(
        &self,
        subject: &SubjectId,
    ) -> Result<RegistrationDecision, StoreError> {
        if self.spend.is_none() && self.registration_capacity.is_none() {
            return Ok(RegistrationDecision::Admitted);
        }
        // Keep the inherited directory read first. A known owner does not need
        // a readable platform state to sign in.
        let directory_error = if let Some(records) = self.records.as_ref() {
            match records.users.user(subject).await {
                Ok(Some(_)) => return Ok(RegistrationDecision::Existing),
                Ok(None) => None,
                Err(error) => Some(error),
            }
        } else {
            None
        };
        let snapshot = match self.spend.as_ref() {
            Some(spend) => Some(spend.admission_snapshot(now_seconds()).await),
            None => None,
        };
        // Resolve retained evidence before honoring a closed or unreadable
        // snapshot, including owners whose directory observation once failed.
        if let Some(capacity) = self.registration_capacity.as_ref() {
            if capacity.existing_subject(subject).await? {
                return Ok(RegistrationDecision::Existing);
            }
        }
        if let Some(error) = directory_error {
            return Err(error);
        }
        let limit = if let Some(snapshot) = snapshot {
            let (state, ceilings) =
                snapshot.map_err(|error| StoreError::Unavailable(error.to_string()))?;
            if registration_decision(&state, false) == RegistrationDecision::Closed {
                return Ok(RegistrationDecision::Closed);
            }
            match ceilings {
                Ok(ceilings) => Some(Ok(ceilings.registrations)),
                Err(()) => return Ok(RegistrationDecision::Closed),
            }
        } else {
            None
        };
        if let Some(capacity) = self.registration_capacity.as_ref() {
            let accepted = match limit {
                Some(limit) => capacity.admit_with_limit(subject, limit).await?,
                None => capacity.admit(subject).await?,
            };
            if !accepted {
                return Ok(RegistrationDecision::Closed);
            }
        }
        Ok(RegistrationDecision::Admitted)
    }

    /// Advisory, at the sign-up entry: an unreadable state proceeds and the
    /// callback decides, because an identity can also be created upstream.
    pub(super) async fn registration_open(&self) -> bool {
        let limit = if let Some(spend) = self.spend.as_ref() {
            match spend.admission_snapshot(now_seconds()).await {
                Ok((state, ceilings)) => {
                    if state.closed_for_registration() {
                        return false;
                    }
                    match ceilings {
                        Ok(ceilings) => Some(Ok(ceilings.registrations)),
                        Err(()) => return false,
                    }
                }
                // Preserve the existing advisory behavior for a spend read
                // failure; capacity still performs its own fail-closed check.
                Err(_) => None,
            }
        } else {
            None
        };
        match self.registration_capacity.as_ref() {
            Some(capacity) => match limit {
                Some(limit) => capacity.available_with_limit(limit).await.unwrap_or(false),
                None => capacity.available().await.unwrap_or(false),
            },
            None => true,
        }
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
