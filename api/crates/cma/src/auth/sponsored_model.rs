//! Exact session credential authentication for the sponsored model data plane.

use std::time::SystemTime;

use layer2_core::store::StoreError;
use layer2_core::SessionId;

use super::AuthState;

impl AuthState {
    /// Authenticate the independently delivered credential currently held in
    /// durable custody for one session.
    ///
    /// This intentionally does not resolve a subject, browser token, delegated
    /// bearer, or ordinary capability. Sponsored spend requires this exact
    /// factor and the separate inert grant capability; neither authorizes the
    /// other.
    pub(crate) async fn authenticates_session_credential(
        &self,
        session: &SessionId,
        custody_generation: u64,
        bearer: &str,
    ) -> Result<bool, StoreError> {
        if custody_generation == 0 || bearer.is_empty() {
            return Ok(false);
        }
        let Some(state) = self.store.credential(session).await? else {
            return Ok(false);
        };
        state.validate()?;
        if state.retired {
            return Ok(false);
        }
        let Some(current) = state.current.as_ref() else {
            return Ok(false);
        };
        if current.custody_generation != custody_generation
            || current.custody_generation == 0
            || current
                .expires_at
                .is_some_and(|expires_at| expires_at <= SystemTime::now())
        {
            return Ok(false);
        }
        let comparison_key = ring::hmac::Key::new(
            ring::hmac::HMAC_SHA256,
            b"cma-sponsored-session-credential-comparison-v1",
        );
        let expected = ring::hmac::sign(&comparison_key, current.value.expose().as_bytes());
        Ok(ring::hmac::verify(&comparison_key, bearer.as_bytes(), expected.as_ref()).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use layer2_core::{Authorization, SubjectId};

    use super::*;
    use crate::auth::persistence::{
        AuthStore, DurableCredential, MemoryAuthStore, SecretValue, SessionCredentialState,
    };
    use crate::auth::AuthMode;

    fn credential_state(
        session: &SessionId,
        value: &str,
        custody_generation: u64,
        expires_at: SystemTime,
    ) -> SessionCredentialState {
        SessionCredentialState {
            session: session.clone(),
            generation: 0,
            request_bound_credential: None,
            current: Some(DurableCredential {
                id: "session-provider-key".into(),
                value: SecretValue::new(value.into()),
                owner: SubjectId("contributor".into()),
                issued_at: SystemTime::now(),
                expires_at: Some(expires_at),
                scope_fingerprint: "scope".into(),
                custody_generation,
                provider_state_version: 1,
                grant_fingerprint: "grant".into(),
                revocation_credential: None,
            }),
            pending: None,
            pending_target: None,
            revoke_debt: Vec::new(),
            lease: None,
            retired: false,
        }
    }

    fn state(store: Arc<MemoryAuthStore>) -> Arc<AuthState> {
        let _ = Authorization::none();
        AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store,
        )
    }

    #[tokio::test]
    async fn only_the_exact_live_credential_and_custody_generation_authenticate() {
        let session = SessionId("sesn_exact_sponsored_factor".into());
        let store = Arc::new(MemoryAuthStore::default());
        store
            .insert_credential(credential_state(
                &session,
                "exact-session-secret",
                7,
                SystemTime::now() + Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let auth = state(store);

        assert!(auth
            .authenticates_session_credential(&session, 7, "exact-session-secret")
            .await
            .unwrap());
        assert!(!auth
            .authenticates_session_credential(&session, 6, "exact-session-secret")
            .await
            .unwrap());
        assert!(!auth
            .authenticates_session_credential(&session, 7, "wrong-session-secret")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn an_expired_session_credential_never_authenticates() {
        let session = SessionId("sesn_expired_sponsored_factor".into());
        let store = Arc::new(MemoryAuthStore::default());
        store
            .insert_credential(credential_state(
                &session,
                "expired-session-secret",
                8,
                SystemTime::now() - Duration::from_secs(1),
            ))
            .await
            .unwrap();
        let auth = state(store);

        assert!(!auth
            .authenticates_session_credential(&session, 8, "expired-session-secret")
            .await
            .unwrap());
    }
}
