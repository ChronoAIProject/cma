//! Accepting an identity another service minted.
//!
//! Split from the state so that file is about this service's own sign-in flow
//! and this one about the other way a caller arrives: holding a token issued
//! elsewhere in the platform.
//!
//! The distinction is worth a file because the trust is different. A session
//! cookie is something this service issued and can revoke; a bearer token is
//! somebody else's assertion, verified against their keys, and adopted only on
//! terms this service sets.

use std::time::{Duration, SystemTime};

use layer2_core::{store::StoreError, Authorization, SubjectId};

use super::persistence::{DurableSubjectTokens, SecretValue};
use super::{next_token_generation, AuthState};

impl AuthState {
    /// Who a bearer token belongs to, asking the provider when necessary.
    ///
    /// The provider is the only thing that can answer this. Decoding the token
    /// here would accept anything shaped like one, and would keep accepting it
    /// after it had been revoked.
    pub(super) async fn subject_for_bearer(
        &self,
        token: &str,
    ) -> Result<Option<SubjectId>, StoreError> {
        if let Some((subject, authorization, profile)) = self.bearer.get(token).await {
            self.adopt_bearer(&subject, authorization, profile, token)
                .await?;
            return Ok(Some(subject));
        }
        let Ok(meta) = self.metadata().await else {
            return Ok(None);
        };
        match crate::auth::flow::resolve_subject(self, &meta, token).await {
            Ok((subject, permissions, profile)) => {
                let authorization = crate::permissions::capabilities_from_permissions(&permissions);
                self.bearer
                    .remember(
                        token,
                        subject.clone(),
                        authorization.clone(),
                        profile.clone(),
                    )
                    .await;
                self.adopt_bearer(&subject, authorization, profile, token)
                    .await?;
                Ok(Some(subject))
            }
            Err(e) => {
                // Not an error worth a stack trace: an expired or withdrawn
                // token is an ordinary thing for a client to present.
                tracing::debug!(error = %e, "a bearer token was not accepted");
                Ok(None)
            }
        }
    }

    /// Make a bearer caller indistinguishable from a signed-in one downstream.
    ///
    /// Everything past the middleware reads capabilities, a profile and a
    /// delegated credential from the same places whichever way the caller
    /// arrived. Keeping the token as the delegated credential is what lets a
    /// session started by a server act on that caller's behalf, exactly as one
    /// started from a browser does.
    async fn adopt_bearer(
        &self,
        subject: &SubjectId,
        authorization: Authorization,
        profile: crate::auth::profile::Profile,
        token: &str,
    ) -> Result<(), StoreError> {
        self.replace_subject_fact(subject, |state| {
            state.authorization = authorization.clone();
            state.profile = profile.clone();
            // Only when this service is not already holding a better one: a
            // token it minted itself can be renewed, and a client's cannot.
            if state
                .tokens
                .as_ref()
                .is_none_or(|tokens| tokens.refresh_token.is_none())
            {
                state.token_generation = next_token_generation(state.token_generation)?;
                state.tokens = Some(DurableSubjectTokens {
                    access_token: SecretValue::new(token.to_string()),
                    refresh_token: None,
                    // The provider decides when the token really expires; this
                    // is the period for which the bearer cache may reuse it.
                    expires_at: SystemTime::now() + Duration::from_secs(60),
                });
                state.refresh_lease = None;
            }
            Ok(())
        })
        .await
    }
}
