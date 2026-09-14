//! Aggregate-neutral records written after a successful sign-in.

use std::sync::Arc;

use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget,
    PublicIdentity, SubjectId, UserDirectory,
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
