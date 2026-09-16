use crate::resource_access::ResourceOwner;
use serde::Deserialize;

use super::{
    identifier, CallbackError, CallbackKeySet, CallbackVerifier, RelayCorroboration, StandardClaims,
};

#[derive(Deserialize)]
struct RelayClaims {
    #[serde(flatten)]
    standard: StandardClaims,
    sub: String,
    relay: bool,
    relay_api_key_id: String,
    relay_allowed_service_ids: Vec<String>,
    relay_allowed_node_ids: Vec<String>,
    relay_allow_all_services: bool,
    relay_allow_all_nodes: bool,
    act: Option<serde_json::Value>,
    delegated: Option<bool>,
    sa: Option<bool>,
    cnf: Option<serde_json::Value>,
    assistant_forward: Option<bool>,
}

impl CallbackVerifier {
    pub(super) fn relay(
        &self,
        keys: &CallbackKeySet,
        token: Option<&str>,
        owner: &ResourceOwner,
        key: &str,
        now: i64,
    ) -> Result<RelayCorroboration, CallbackError> {
        let Some(token) = token else {
            return Ok(RelayCorroboration::AbsentInboxOnly);
        };
        let (claims, _): (RelayClaims, _) = keys.verify(token, &self.policy.issuer)?;
        claims.standard.require(
            &self.policy,
            &self.policy.relay_audience,
            "access",
            self.policy.relay_ttl_seconds,
            now,
        )?;
        let subject = match owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if !claims.relay
            || claims.sub != subject.as_str()
            || claims.relay_api_key_id != key
            || claims.act.is_some()
            || claims.delegated.unwrap_or(false)
            || claims.sa.unwrap_or(false)
            || claims.cnf.is_some()
            || claims.assistant_forward.unwrap_or(false)
            || [
                &claims.relay_allowed_service_ids,
                &claims.relay_allowed_node_ids,
            ]
            .iter()
            .any(|ids| ids.len() > 256 || ids.iter().any(|id| !identifier(id)))
        {
            return Err(CallbackError::Authentication);
        }
        // Scope flags are parsed strictly and discarded: this foundation never
        // turns inherited relay scope into a usable execution credential.
        let _ = (
            claims.relay_allow_all_services,
            claims.relay_allow_all_nodes,
        );
        Ok(RelayCorroboration::VerifiedInboxOnly {
            subject: claims.sub,
            key_id: claims.relay_api_key_id,
            expires_at: claims.standard.exp,
        })
    }
}
