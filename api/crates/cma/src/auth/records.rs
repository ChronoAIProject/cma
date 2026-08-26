//! Writing down that somebody signed in.
//!
//! Kept beside the flow rather than inside it because sign-in is the moment the
//! provider's account details are deliberately recorded. Later authorization
//! checks use userinfo only to reconcile the security-sensitive grant; they do
//! not create another sign-in event or rewrite this directory entry.
//!
//! Two records, deliberately. The directory answers "who uses this", which is
//! a question about people and has one entry each. The account of actions
//! answers "what happened", which is a question about events and has one entry
//! per sign-in. Collapsing them would lose one or the other.

use std::sync::Arc;

use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, SubjectId,
    UserDirectory,
};

use super::profile::Profile;
use super::AuthState;

/// Where a sign-in is written down.
///
/// Held together because they are written together and a deployment that has
/// one without the other would record half of each sign-in.
pub struct SignInRecords {
    /// Who has used the service.
    pub users: Arc<UserDirectory>,
    /// What the service was asked to do.
    pub activity: Arc<ActivityService>,
}

impl AuthState {
    /// Note that a subject signed in, with whatever the provider released.
    ///
    /// Never fails and never blocks a sign-in: somebody who has authenticated
    /// has authenticated, and turning them away because the note about it
    /// could not be written would be a worse outcome than the missing note.
    /// Each half reports its own failure.
    ///
    /// A deployment with nothing to write to — local bring-up against a fixed
    /// subject, where there is no sign-in to speak of — records nothing rather
    /// than pretending to.
    pub async fn note_sign_in(&self, subject: &SubjectId, profile: &Profile) {
        let Some(records) = self.records.as_ref() else {
            tracing::debug!(%subject, "no record is kept of who signs in");
            return;
        };
        records
            .users
            .note_sign_in(
                subject,
                profile.display_name.clone(),
                profile.email.clone(),
                profile.avatar_url.clone(),
            )
            .await;
        records
            .activity
            .record(
                ActivityRecord::new(ActivityAction::SignIn, ActivityOutcome::Succeeded)
                    .by(subject.clone())
                    // Its own subject as the target: a sign-in is an action
                    // taken on a person, and it reads the same as an operator
                    // acting on somebody else does.
                    .on(ActivityTarget::Subject(subject.clone())),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMode;

    #[tokio::test]
    async fn a_deployment_that_keeps_no_record_still_signs_people_in() {
        // Local bring-up has no store behind it and no sign-in to record. It
        // must not panic, and it must not refuse anybody.
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local-developer".into(),
            },
            None,
        );
        auth.note_sign_in(
            &SubjectId("local-developer".into()),
            &Profile {
                display_name: Some("Local Developer".into()),
                email: None,
                avatar_url: None,
            },
        )
        .await;
    }
}
