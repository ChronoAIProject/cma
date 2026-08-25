//! Where a subject's resolved capabilities are kept.
//!
//! Separated from token custody so each file is about one thing: this answers
//! "what may they do", `super` answers "who are they and can we still act for
//! them". They are resolved once at sign-in and looked up per request.

use layer2_core::{store::StoreError, Authorization, SubjectId};

use super::{AuthMode, AuthState};

impl AuthState {
    /// What a subject may do in this deployment.
    ///
    /// One answer rather than two: the middleware that enforces capabilities
    /// and the endpoint that reports them read the same function, so a client
    /// can never be told it may do something the next request refuses. That
    /// disagreement is the whole failure mode of telling a client anything.
    pub async fn effective_authorization(
        &self,
        subject: &SubjectId,
    ) -> Result<Authorization, StoreError> {
        match self.mode {
            AuthMode::Trusted { .. } => Ok(Self::trusted_authorization()),
            AuthMode::Provider { .. } => self.authorization(subject).await,
        }
    }

    /// Record what a subject may do, as resolved at sign-in.
    pub async fn store_authorization(
        &self,
        subject: &SubjectId,
        authorization: Authorization,
    ) -> Result<(), StoreError> {
        self.replace_subject_fact(subject, |state| {
            state.authorization = authorization.clone();
            Ok(())
        })
        .await
    }

    /// What a subject may do.
    ///
    /// A subject with nothing recorded gets an empty authorization rather than
    /// an error, so an unknown caller is denied by every check instead of
    /// bypassing them.
    pub async fn authorization(&self, subject: &SubjectId) -> Result<Authorization, StoreError> {
        Ok(self
            .subject_fact(subject)
            .await?
            .map(|state| state.authorization)
            .unwrap_or_else(Authorization::none))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMode;
    use layer2_core::Capability;

    fn provider_state() -> std::sync::Arc<AuthState> {
        AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        )
    }

    #[tokio::test]
    async fn an_unknown_subject_is_denied_rather_than_admitted() {
        // The lookup must fail closed: a subject with nothing recorded gets an
        // empty authorization, not a bypass.
        let auth = provider_state();
        let a = auth
            .authorization(&SubjectId("never-seen".into()))
            .await
            .expect("memory store works");
        assert_eq!(a.capabilities().count(), 0);
        assert!(!a.allows(Capability::CreateSession));
    }

    #[tokio::test]
    async fn a_stored_authorization_is_returned() {
        let auth = provider_state();
        let s = SubjectId("subject-a".into());
        auth.store_authorization(
            &s,
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        )
        .await
        .expect("authorization persists");
        let a = auth.authorization(&s).await.expect("authorization loads");
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(!a.allows(Capability::ReadAllSessions));
    }

    #[tokio::test]
    async fn re_signing_in_replaces_rather_than_merges() {
        // Capabilities revoked at the provider must not survive in our copy.
        let auth = provider_state();
        let s = SubjectId("subject-a".into());
        auth.store_authorization(
            &s,
            Authorization::from_capabilities([Capability::ReadAllSessions]),
        )
        .await
        .expect("first authorization persists");
        auth.store_authorization(
            &s,
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        )
        .await
        .expect("replacement authorization persists");
        let a = auth.authorization(&s).await.expect("authorization loads");
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(
            !a.allows(Capability::ReadAllSessions),
            "a revoked capability must not persist"
        );
    }

    #[tokio::test]
    async fn trusted_mode_answers_the_same_whoever_asks() {
        // Local bring-up has no provider to resolve against, so the fixed
        // subject carries the everyday set — and the report a client reads has
        // to say exactly that, or the console would offer nothing at all.
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local-developer".into(),
            },
            None,
        );
        let a = auth
            .effective_authorization(&SubjectId("anybody".into()))
            .await
            .expect("trusted authorization is available");
        assert!(a.allows(Capability::CreateSession));
        assert!(!a.is_privileged());
    }

    #[tokio::test]
    async fn provider_mode_reports_what_was_resolved_at_sign_in() {
        let auth = provider_state();
        let s = SubjectId("subject-a".into());
        auth.store_authorization(
            &s,
            Authorization::from_capabilities([Capability::ReadActivityLog]),
        )
        .await
        .expect("authorization persists");
        assert!(auth
            .effective_authorization(&s)
            .await
            .expect("authorization loads")
            .allows(Capability::ReadActivityLog));
        // And an unknown subject still gets nothing, which is what makes the
        // report safe to hand to a browser.
        assert_eq!(
            auth.effective_authorization(&SubjectId("never-seen".into()))
                .await
                .expect("authorization loads")
                .capabilities()
                .count(),
            0
        );
    }

    #[test]
    fn trusted_mode_grants_daily_use_but_not_the_admin_surface() {
        let a = AuthState::trusted_authorization();
        assert!(a.allows(Capability::CreateSession));
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(
            !a.is_privileged(),
            "local bring-up must not silently grant the admin surface"
        );
    }
}
