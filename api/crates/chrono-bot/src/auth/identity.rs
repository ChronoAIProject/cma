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

use layer2_core::{Authorization, SubjectId};

use super::{AuthState, SubjectTokens};

impl AuthState {
    /// Who a bearer token belongs to, asking the provider when necessary.
    ///
    /// The provider is the only thing that can answer this. Decoding the token
    /// here would accept anything shaped like one, and would keep accepting it
    /// after it had been revoked.
    pub(super) async fn subject_for_bearer(&self, token: &str) -> Option<SubjectId> {
        if let Some((subject, authorization, profile)) = self.bearer.get(token).await {
            self.adopt_bearer(&subject, authorization, profile, token)
                .await;
            return Some(subject);
        }
        let meta = self.metadata().await.ok()?;
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
                    .await;
                Some(subject)
            }
            Err(e) => {
                // Not an error worth a stack trace: an expired or withdrawn
                // token is an ordinary thing for a client to present.
                tracing::debug!(error = %e, "a bearer token was not accepted");
                None
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
    ) {
        self.authorizations
            .write()
            .await
            .insert(subject.clone(), authorization);
        self.profiles.write().await.insert(subject.clone(), profile);
        let mut tokens = self.tokens.write().await;
        let entry = tokens
            .entry(subject.clone())
            .or_insert_with(|| SubjectTokens {
                access_token: token.to_string(),
                refresh_token: None,
                // Held for as long as this service will reuse the answer about it.
                // The provider decides when the token really expires; this only
                // says when to stop assuming it has not been withdrawn.
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
            });
        // Only when this service is not already holding a better one: a token
        // it minted itself can be renewed, and a client's cannot.
        if entry.refresh_token.is_none() {
            entry.access_token = token.to_string();
        }
    }
}
