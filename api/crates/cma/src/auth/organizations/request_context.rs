use layer2_core::resource_access::{CallerContext, CredentialContext, ResourceActor};
use layer2_core::Caller;

use super::{transport, AuthMode, AuthState, SecretDigest};
use crate::auth::RequestAuth;

impl AuthState {
    /// Project only identity already authenticated by the request resolver. No
    /// organization authority is inferred here and no provider IO is required.
    pub(crate) fn resource_request_context(&self, resolved: &RequestAuth) -> CallerContext {
        let credential = match (&self.mode, &resolved.delegated_credential) {
            (AuthMode::Provider { base_url, .. }, Some(value)) => CredentialContext {
                issuer: base_url.trim_end_matches('/').into(),
                subject: resolved.subject.clone(),
                credential_id: transport::digest_id(SecretDigest::identify(
                    "organization-caller",
                    value,
                )),
            },
            _ => CredentialContext {
                issuer: "cma:local-request".into(),
                subject: resolved.subject.clone(),
                credential_id: uuid::Uuid::new_v4().to_string(),
            },
        };
        CallerContext {
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
