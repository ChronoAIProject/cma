//! Where a subject's resolved capabilities are kept.
//!
//! Separated from token custody so each file is about one thing: this answers
//! "what may they do", `super` answers "who are they and can we still act for
//! them". They are resolved once at sign-in and looked up per request.

use layer2_core::{Authorization, SubjectId};

use super::AuthState;

impl AuthState {
    /// Record what a subject may do, as resolved at sign-in.
    pub async fn store_authorization(&self, subject: &SubjectId, authorization: Authorization) {
        self.authorizations
            .write()
            .await
            .insert(subject.clone(), authorization);
    }

    /// What a subject may do.
    ///
    /// A subject with nothing recorded gets an empty authorization rather than
    /// an error, so an unknown caller is denied by every check instead of
    /// bypassing them.
    pub async fn authorization(&self, subject: &SubjectId) -> Authorization {
        self.authorizations
            .read()
            .await
            .get(subject)
            .cloned()
            .unwrap_or_else(Authorization::none)
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
        let a = auth.authorization(&SubjectId("never-seen".into())).await;
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
        .await;
        let a = auth.authorization(&s).await;
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
        .await;
        auth.store_authorization(
            &s,
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        )
        .await;
        let a = auth.authorization(&s).await;
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(
            !a.allows(Capability::ReadAllSessions),
            "a revoked capability must not persist"
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
