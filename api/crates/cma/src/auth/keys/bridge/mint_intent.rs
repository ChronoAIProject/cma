//! Nonsecret proof of the exact one-shot request, retained before dispatch.
//! This proof never grants permission to replay a mint or recover its secret.
use super::{BridgeSandboxKeyPlan, KeyIssuerError};
mod reconcile;
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{
    bridge::{BridgeConnector, BridgeId},
    resource_access::ResourceOwner,
    sandbox::SandboxCredentialAuthority,
    SubjectId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeSandboxMintIntent {
    schema_version: u16,
    bridge_id: BridgeId,
    connector: BridgeConnector,
    owner: ResourceOwner,
    actor: SubjectId,
    grant_digest: String,
    deployment_id: String,
    credential_id: String,
    expires_at: String,
    scope_plan_digest: String,
    grant_fingerprint: String,
}

impl BridgeSandboxKeyPlan {
    pub(crate) fn mint_intent(&self) -> Result<BridgeSandboxMintIntent, KeyIssuerError> {
        let grant_fingerprint =
            super::super::provider_grant_fingerprint(&super::super::MintedKeyResponse {
                id: String::new(),
                full_key: String::new(),
                scopes: super::super::KEY_SCOPES.into(),
                state_version: 1,
                allowed_service_ids: self.scope.allowed_service_ids.clone(),
                allowed_node_ids: self.scope.allowed_node_ids.clone(),
                allow_all_services: false,
                allow_all_nodes: false,
            })
            .ok_or(KeyIssuerError::UnexpectedAuthority)?;
        Ok(BridgeSandboxMintIntent {
            schema_version: 1,
            bridge_id: self.bridge_id.clone(),
            connector: self.connector.clone(),
            owner: self.owner.clone(),
            actor: self.actor.clone(),
            grant_digest: digest(&self.grant)?,
            deployment_id: self.deployment_id.clone(),
            credential_id: self.credential_id.clone(),
            expires_at: layer2_core::rfc3339_at(self.expires_at),
            scope_plan_digest: self.scope.normalized_grant_digest.clone(),
            grant_fingerprint,
        })
    }
}

impl BridgeSandboxMintIntent {
    pub(crate) fn require_binding(
        &self,
        lease: &BridgeControlPlaneLease,
        deployment: &str,
        credential_id: &str,
    ) -> Result<(), KeyIssuerError> {
        self.validate()?;
        super::require_lease(lease)?;
        if self.bridge_id != *lease.bridge_id()
            || self.connector != *lease.connector()
            || self.owner != *lease.owner()
            || self.actor != *lease.actor_subject()
            || self.grant_digest != digest(lease.grant())?
            || self.deployment_id != deployment
            || self.credential_id != credential_id
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(())
    }

    pub(crate) fn require_material(
        &self,
        material: &SandboxCredentialAuthority,
    ) -> Result<(), KeyIssuerError> {
        self.validate()?;
        if material.provider_state_version <= 0
            || !super::super::valid_provider_key_id(&material.provider_key_id)
            || material.expires_at != self.expires_at
            || material.grant_fingerprint != self.grant_fingerprint
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<(), KeyIssuerError> {
        super::super::validate_credential_id(&self.credential_id)?;
        super::super::key_name(&self.deployment_id, &self.credential_id)?;
        if self.schema_version != 1
            || self.bridge_id.as_str().is_empty()
            || self.actor.as_str().is_empty()
            || layer2_core::seconds_from_rfc3339(&self.expires_at).is_none()
            || [
                &self.grant_digest,
                &self.grant_fingerprint,
                &self.scope_plan_digest,
            ]
            .into_iter()
            .any(|value| {
                value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(())
    }
}

fn digest(value: &impl Serialize) -> Result<String, KeyIssuerError> {
    serde_json::to_vec(value)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| KeyIssuerError::UnexpectedAuthority)
}
