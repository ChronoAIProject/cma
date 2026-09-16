//! Read-only correlation of one durable mint intent. Absence never licenses a
//! second POST: the original request may still be executing at the provider.
use super::BridgeSandboxMintIntent;
#[cfg(test)]
mod tests;
use crate::{
    auth::keys::{bridge::renewal::PriorKeyMetadata, KeyIssuer, KeyIssuerError},
    bridge::BridgeControlPlaneLease,
};
use layer2_core::{resource_access::ResourceOwner, sandbox::SandboxCredentialAuthority};
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
    keys: Vec<Key>,
}
#[derive(Deserialize)]
struct Key {
    id: String,
    name: String,
    created_at: String,
}

impl KeyIssuer {
    /// Returns nonsecret exact-key evidence only. The caller must durably bind
    /// it to the original intent before a separately authorized scoped DELETE.
    pub(crate) async fn observe_bridge_sandbox_mint(
        &self,
        lease: &BridgeControlPlaneLease,
        intent: &BridgeSandboxMintIntent,
        generation: u64,
    ) -> Result<Option<SandboxCredentialAuthority>, KeyIssuerError> {
        intent.require_binding(lease, &self.deployment, &intent.credential_id)?;
        if generation == 0 {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let mut request = self.http.get(format!(
            "{}{}",
            self.base_url,
            super::super::super::KEYS_PATH
        ));
        if let ResourceOwner::Organization { subject } = &intent.owner {
            request = request.query(&[("org_id", subject.as_str())]);
        }
        let listing: Listing = self.bridge_json(request, lease).await?;
        if listing.keys.len() > super::super::super::PROVIDER_KEY_LIST_MAX_ROWS {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let expected_name = super::super::super::key_name(&self.deployment, &intent.credential_id)?;
        let mut ids = BTreeSet::new();
        let mut candidate = None;
        for key in listing.keys {
            if !super::super::super::valid_provider_key_id(&key.id) || !ids.insert(key.id.clone()) {
                return Err(KeyIssuerError::UnexpectedAuthority);
            }
            if key.name == expected_name && candidate.replace(key).is_some() {
                return Err(KeyIssuerError::UnexpectedAuthority);
            }
        }
        let Some(key) = candidate else {
            intent.require_binding(lease, &self.deployment, &intent.credential_id)?;
            return Ok(None);
        };
        let issued_at = layer2_core::seconds_from_rfc3339(&key.created_at)
            .ok_or(KeyIssuerError::UnexpectedAuthority)?;
        if issued_at
            >= layer2_core::seconds_from_rfc3339(&intent.expires_at)
                .ok_or(KeyIssuerError::UnexpectedAuthority)?
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let observed = SandboxCredentialAuthority {
            generation,
            provider_key_id: key.id,
            // General key creation starts at 1. Mutated candidates stay held.
            provider_state_version: 1,
            issued_at: key.created_at,
            expires_at: intent.expires_at.clone(),
            grant_fingerprint: intent.grant_fingerprint.clone(),
        };
        let metadata: PriorKeyMetadata = self
            .bridge_json(
                self.http.get(format!(
                    "{}{}/{}",
                    self.base_url,
                    super::super::super::KEYS_PATH,
                    observed.provider_key_id
                )),
                lease,
            )
            .await?;
        metadata.require_initial_version()?;
        metadata.require(lease, &expected_name, &observed)?;
        intent.require_binding(lease, &self.deployment, &intent.credential_id)?;
        Ok(Some(observed))
    }
}
