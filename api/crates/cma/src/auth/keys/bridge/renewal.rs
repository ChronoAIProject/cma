//! Revocation uses the retained human grant and exact prior-key metadata. An
//! organization key is never revoked using a personal owner-subject lookup.
use super::{require_lease, unavailable, KeyIssuer, KeyIssuerError};
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{resource_access::ResourceOwner, sandbox::SandboxCredentialAuthority};
use serde::Deserialize;
mod absence;
#[cfg(test)]
mod tests;

impl KeyIssuer {
    pub(crate) async fn revoke_bridge_sandbox_key(
        &self,
        lease: &BridgeControlPlaneLease,
        credential_id: &str,
        previous: &SandboxCredentialAuthority,
    ) -> Result<(), KeyIssuerError> {
        require_lease(lease)?;
        if !super::super::valid_provider_key_id(&previous.provider_key_id) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let name = super::super::key_name(&self.deployment, credential_id)?;
        let url = format!(
            "{}{}/{}",
            self.base_url,
            super::super::KEYS_PATH,
            previous.provider_key_id
        );
        let metadata: PriorKeyMetadata = match self.bridge_json(self.http.get(&url), lease).await {
            Ok(metadata) => metadata,
            // Detail404 also conceals lost org membership. Require fresh
            // original-owner list permission and exact absence before receipt.
            Err(KeyIssuerError::Refused(404)) => {
                return self
                    .require_bridge_key_absent(lease, &previous.provider_key_id)
                    .await;
            }
            Err(error) => return Err(error),
        };
        metadata.require(lease, &name, previous)?;
        require_lease(lease)?;
        // DELETE resolves the exact key owner server-side; its API does not
        // accept target_org_id. The GET above proves the original owner.
        let response = tokio::time::timeout_at(
            lease.deadline(),
            self.http.delete(url).bearer_auth(lease.credential()).send(),
        )
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;
        if response.status().as_u16() == 404 {
            return self
                .require_bridge_key_absent(lease, &previous.provider_key_id)
                .await;
        }
        if !response.status().is_success() {
            return Err(KeyIssuerError::Refused(response.status().as_u16()));
        }
        require_lease(lease)
    }
}

#[derive(Deserialize)]
pub(super) struct PriorKeyMetadata {
    id: String,
    name: String,
    is_active: bool,
    expires_at: String,
    state_version: i64,
    scopes: String,
    allowed_service_ids: Vec<String>,
    allowed_node_ids: Vec<String>,
    allow_all_services: bool,
    allow_all_nodes: bool,
    credential_source: PriorSource,
    purpose: String,
    scheduled_write_enabled: bool,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PriorSource {
    Personal,
    Org {
        org_id: String,
        role: String,
        allowed: bool,
    },
}
impl PriorKeyMetadata {
    pub(super) fn require(
        &self,
        lease: &BridgeControlPlaneLease,
        name: &str,
        previous: &SandboxCredentialAuthority,
    ) -> Result<(), KeyIssuerError> {
        let source_matches = match (&self.credential_source, lease.owner()) {
            (PriorSource::Personal, ResourceOwner::Person { subject }) => {
                subject == lease.actor_subject()
            }
            (
                PriorSource::Org {
                    org_id,
                    role,
                    allowed,
                },
                ResourceOwner::Organization { subject },
            ) => org_id == subject.as_str() && role == "admin" && *allowed,
            _ => false,
        };
        let retained_expiry = mongodb::bson::DateTime::parse_rfc3339_str(&previous.expires_at).ok();
        let observed_expiry = mongodb::bson::DateTime::parse_rfc3339_str(&self.expires_at).ok();
        let fingerprint =
            super::super::provider_grant_fingerprint(&super::super::MintedKeyResponse {
                id: self.id.clone(),
                full_key: String::new(),
                scopes: self.scopes.clone(),
                state_version: self.state_version,
                allowed_service_ids: self.allowed_service_ids.clone(),
                allowed_node_ids: self.allowed_node_ids.clone(),
                allow_all_services: self.allow_all_services,
                allow_all_nodes: self.allow_all_nodes,
            });
        if !source_matches
            || self.id != previous.provider_key_id
            || self.name != name
            || !self.is_active
            || self.state_version != previous.provider_state_version
            || retained_expiry.is_none()
            || observed_expiry != retained_expiry
            || self.scopes != super::super::KEY_SCOPES
            || self.purpose != "general"
            || self.scheduled_write_enabled
            || self.allow_all_services
            || self.allow_all_nodes
            || fingerprint.as_deref() != Some(previous.grant_fingerprint.as_str())
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(())
    }
}
