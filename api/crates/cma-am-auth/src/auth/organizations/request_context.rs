use layer2_core::resource_access::{CallerContext, CredentialContext, ResourceActor};
use layer2_core::Caller;

use super::transport;
use crate::auth::RequestAuth;
use crate::auth::{persistence::SecretDigest, AuthMode, AuthState};

impl AuthState {
    pub fn provider_credential_context(
        &self,
        subject: &layer2_core::SubjectId,
        credential: &str,
    ) -> Option<CredentialContext> {
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return None;
        };
        Some(CredentialContext {
            issuer: base_url.trim_end_matches('/').into(),
            subject: subject.clone(),
            credential_id: transport::digest_id(SecretDigest::identify(
                "organization-caller",
                credential,
            )),
        })
    }

    /// Project only identity already authenticated by the request resolver. No
    /// organization authority is inferred here and no provider IO is required.
    pub fn resource_request_context(&self, resolved: &RequestAuth) -> CallerContext {
        if let Some(principal) = &resolved.binding {
            return crate::auth::binding::context(principal);
        }
        let credential = match (&self.mode, &resolved.delegated_credential) {
            (AuthMode::Provider { .. }, Some(value)) => self
                .provider_credential_context(&resolved.subject, value)
                .expect("provider mode is matched"),
            _ => CredentialContext {
                issuer: "cma:local-request".into(),
                subject: resolved.subject.clone(),
                credential_id: uuid::Uuid::new_v4().to_string(),
            },
        };
        CallerContext {
            credential_source: resolved.credential_source,
            caller: Caller::new(resolved.subject.clone(), resolved.authorization.clone()),
            credential,
            actor: ResourceActor::Api {
                subject: resolved.subject.clone(),
                delegation_id: None,
            },
            memberships: Vec::new(),
        }
    }
}
