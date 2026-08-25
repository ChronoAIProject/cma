//! Accepting an identity another service minted.
//!
//! Split from the state so that file is about this service's own sign-in flow
//! and this one about the other way a caller arrives: holding a token issued
//! elsewhere in the platform. Its grants, display claims, and delegated value
//! stay attached to that one request; they are never promoted into renewable
//! browser state merely because another credential names the same subject.
//!
//! The distinction is worth a file because the trust is different. A session
//! cookie is something this service issued and can revoke; a bearer token is
//! somebody else's assertion, verified against their keys, and adopted only on
//! terms this service sets.

use layer2_core::store::StoreError;

use super::{AuthState, RequestAuth};

impl AuthState {
    /// Who a bearer token belongs to, asking the provider when necessary.
    ///
    /// The provider is the only thing that can answer this. Decoding the token
    /// here would accept anything shaped like one, and would keep accepting it
    /// after it had been revoked.
    pub(super) async fn request_for_bearer(
        &self,
        token: String,
    ) -> Result<Option<RequestAuth>, StoreError> {
        if let Some((subject, authorization, profile)) = self.bearer.get(&token).await {
            return Ok(Some(RequestAuth {
                subject,
                authorization,
                profile,
                delegated_credential: Some(token),
            }));
        }
        let Ok(meta) = self.metadata().await else {
            return Ok(None);
        };
        match crate::auth::flow::resolve_subject(self, &meta, &token).await {
            Ok((subject, permissions, profile)) => {
                let authorization = crate::permissions::capabilities_from_permissions(&permissions);
                self.bearer
                    .remember(
                        &token,
                        subject.clone(),
                        authorization.clone(),
                        profile.clone(),
                    )
                    .await;
                Ok(Some(RequestAuth {
                    subject,
                    authorization,
                    profile,
                    delegated_credential: Some(token),
                }))
            }
            Err(e) => {
                // Not an error worth a stack trace: an expired or withdrawn
                // token is an ordinary thing for a client to present.
                tracing::debug!(error = %e, "a bearer token was not accepted");
                Ok(None)
            }
        }
    }
}
