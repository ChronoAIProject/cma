//! Supplied binding keys resolve locally and carry no ambient owner authority.
use super::{profile::Profile, AuthState, RequestAuth};
use layer2_core::resource_access::{
    CallerContext, CredentialContext, CredentialSource, ResourceActor,
};
use layer2_core::sandbox::binding::{BindingAccessService, BindingPrincipal, BindingStore};
use layer2_core::StoreError;
use sha2::{Digest, Sha256};
use std::sync::Arc;

impl AuthState {
    pub(super) async fn is_known_binding_key(&self, token: &str) -> Result<bool, StoreError> {
        let Some(access) = &self.bindings else {
            return Ok(false);
        };
        let hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        access
            .contains_key_hash(&hash)
            .await
            .map_err(|_| StoreError::Unavailable("binding key history unavailable".into()))
    }
    pub fn with_binding_store(mut self: Arc<Self>, store: Arc<dyn BindingStore>) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("binding custody is composed before sharing authentication")
            .bindings = Some(BindingAccessService::new(store));
        self
    }

    pub async fn binding_request(&self, token: String) -> Result<Option<RequestAuth>, StoreError> {
        let access = self.bindings.as_ref().ok_or_else(|| {
            StoreError::Unavailable("binding authentication is not composed".into())
        })?;
        let hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        let Some(principal) = access
            .authenticate(&hash)
            .await
            .map_err(|_| StoreError::Unavailable("binding authentication unavailable".into()))?
        else {
            return Ok(None);
        };
        Ok(Some(RequestAuth {
            credential_source: CredentialSource::Binding,
            subject: principal.owner().clone(),
            authorization: principal.caller().authorization,
            profile: Profile::default(),
            delegated_credential: Some(token),
            binding: Some(principal),
        }))
    }
}

pub(super) fn context(principal: &BindingPrincipal) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Binding,
        caller: principal.caller(),
        credential: CredentialContext {
            issuer: "binding:local".into(),
            subject: principal.owner().clone(),
            credential_id: principal.key_sha256().into(),
        },
        actor: ResourceActor::Api {
            subject: principal.owner().clone(),
            delegation_id: None,
        },
        memberships: vec![],
    }
}

pub(super) fn permits_request(method: &axum::http::Method, path: &str, binding: &str) -> bool {
    use axum::http::Method;
    let segments = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    match (method, segments.as_slice()) {
        (&Method::GET, ["api", "v2"])
        // The setup handler separately attests the configured Father binding
        // and the event's exact sender. This grants no ordinary Bot authority.
        | (&Method::POST, ["api", "v1", "bot-setups", "advance"])
        | (&Method::POST, ["api", "v1", "bot-setups", "actions"])
        | (&Method::POST, ["api", "v1", "agent-delegations", "identity" | "operations"])
        | (&Method::POST, ["api", "v2", "responses"])
        | (&Method::POST, ["api", "v2", "agents", _, "responses"])
        | (&Method::GET, ["api", "v2", "responses", _])
        | (&Method::GET, ["api", "v2", "responses", _, "events"])
        | (&Method::POST, ["api", "v2", "responses", _, "cancel"])
        | (&Method::GET, ["api", "v2", "commands", _])
        | (&Method::GET, ["api", "v2", "conversations", _])
        | (&Method::GET, ["api", "v2", "conversations", _, "items"]) => true,
        (
            method,
            ["api", "v2", "agents", _, "responses", _, "controls" | "queued-cancellation"],
        ) => matches!(*method, Method::GET | Method::POST),
        (
            &Method::GET,
            ["api", "v1", "cmaeg", "bindings", selected, "conversations", _, "events", _],
        ) => *selected == binding,
        (&Method::POST, ["api", "v1", "cmaeg", "bindings", selected, "conversations", _, "events", _, "media", _, "attachment" | "supplement"])
        | (&Method::GET, ["api", "v1", "cmaeg", "bindings", selected, "conversations", _, "events", _, "media", _, "attachment"]) => *selected == binding,
        _ => false,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod command_tests;
