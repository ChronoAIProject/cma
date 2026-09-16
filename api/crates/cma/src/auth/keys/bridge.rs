//! Exact Channel Sandbox keys use an explicitly consented human control-plane
//! lease. Bot keys and owner-subject token lookup cannot enter this path.
mod contract;
#[cfg(test)]
mod http_tests;
mod material;
pub(crate) mod mint_intent;
mod renewal;
#[cfg(test)]
pub(crate) mod test_fixture;
use contract::*;

use super::{KeyIssuer, KeyIssuerError, MintedKey};
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{resource_access::ResourceOwner, ServiceId, SubjectId};

/// Scope planning is read-only. The caller persists a unique mint attempt
/// before sending this plan; any uncertain POST remains held for reconciliation.
pub(crate) struct BridgeSandboxKeyPlan {
    bridge_id: layer2_core::bridge::BridgeId,
    connector: layer2_core::bridge::BridgeConnector,
    owner: ResourceOwner,
    actor: SubjectId,
    grant: crate::bridge::control_plane::BridgeControlPlaneGrant,
    deployment_id: String,
    credential_id: String,
    scope: ScopePlan,
    expires_at: std::time::SystemTime,
    deadline: tokio::time::Instant,
}

impl KeyIssuer {
    pub(crate) async fn plan_bridge_sandbox_key(
        &self,
        lease: &BridgeControlPlaneLease,
        credential_id: &str,
        wanted: &[ServiceId],
    ) -> Result<BridgeSandboxKeyPlan, KeyIssuerError> {
        require_lease(lease)?;
        super::validate_credential_id(credential_id)?;
        if wanted.is_empty() || wanted.len() > 128 {
            return Err(KeyIssuerError::EmptyGrant);
        }
        let services: ServiceList = self
            .bridge_json(
                self.http
                    .get(format!("{}{}", self.base_url, super::USER_SERVICES_PATH)),
                lease,
            )
            .await?;
        let selected = select_services(
            lease.owner(),
            lease.actor_subject(),
            &services.services,
            wanted,
        )?;
        let mut body = serde_json::json!({
            "selected_service_ids": selected,
            "allow_auto_connected_services": false,
        });
        attach_org(&mut body, lease.owner());
        let scope: ScopePlan = self
            .bridge_json(
                self.http
                    .post(format!("{}/api/v1/api-keys/scope-plan", self.base_url))
                    .json(&body),
                lease,
            )
            .await?;
        scope.require(lease.owner(), lease.actor_subject(), &selected)?;
        require_lease(lease)?;
        Ok(BridgeSandboxKeyPlan {
            bridge_id: lease.bridge_id().clone(),
            connector: lease.connector().clone(),
            owner: lease.owner().clone(),
            actor: lease.actor_subject().clone(),
            grant: lease.grant().clone(),
            deployment_id: self.deployment.clone(),
            credential_id: credential_id.into(),
            scope,
            // Custody stores canonical whole-second RFC3339 timestamps. Ask
            // NyxID for that same instant so exact metadata validation survives
            // the durable secret-custody round trip without losing precision.
            expires_at: std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(
                    super::key_expiry(std::time::SystemTime::now())
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| unavailable())?
                        .as_secs(),
                ),
            deadline: lease.deadline(),
        })
    }

    /// Call only after a durable mint-attempt write and current grant fence.
    /// This method never retries the POST. Errors after dispatch mean unknown
    /// creation and require exact deployment/resource-name reconciliation.
    pub(crate) async fn mint_bridge_sandbox_key(
        &self,
        lease: &BridgeControlPlaneLease,
        plan: BridgeSandboxKeyPlan,
    ) -> Result<MintedKey, KeyIssuerError> {
        require_lease(lease)?;
        if plan.deployment_id != self.deployment
            || plan.owner != *lease.owner()
            || plan.actor != *lease.actor_subject()
            || plan.bridge_id != *lease.bridge_id()
            || plan.connector != *lease.connector()
            || plan.grant != *lease.grant()
            || tokio::time::Instant::now() >= plan.deadline
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let mut body = super::mint_request_body(
            &self.deployment,
            &plan.credential_id,
            &plan.scope.allowed_service_ids,
            plan.expires_at,
        )?;
        body["allowed_node_ids"] = serde_json::json!(plan.scope.allowed_node_ids);
        body["allow_auto_connected_services"] = serde_json::Value::Bool(false);
        body["scope_plan_digest"] =
            serde_json::Value::String(plan.scope.normalized_grant_digest.clone());
        attach_org(&mut body, &plan.owner);
        let response: super::MintedKeyResponse = self
            .bridge_json(
                self.http
                    .post(format!("{}{}", self.base_url, super::KEYS_PATH))
                    .json(&body),
                lease,
            )
            .await?;
        let material = material(response, &plan.scope, plan.expires_at)?;
        let metadata: KeyMetadata = self
            .bridge_json(
                self.http.get(format!(
                    "{}{}/{}",
                    self.base_url,
                    super::KEYS_PATH,
                    material.id
                )),
                lease,
            )
            .await?;
        metadata.require(
            &plan.owner,
            &plan.actor,
            &super::key_name(&self.deployment, &plan.credential_id)?,
            &material,
            &plan.scope,
        )?;
        require_lease(lease)?;
        Ok(material)
    }

    async fn bridge_json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        lease: &BridgeControlPlaneLease,
    ) -> Result<T, KeyIssuerError> {
        use futures_util::StreamExt;
        require_lease(lease)?;
        tokio::time::timeout_at(lease.deadline(), async {
            let response = request
                .bearer_auth(lease.credential())
                .send()
                .await
                .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(KeyIssuerError::Refused(response.status().as_u16()));
            }
            let mut bytes = zeroize::Zeroizing::new(Vec::new());
            if response
                .content_length()
                .is_some_and(|size| size > 1_048_576)
            {
                return Err(unavailable());
            }
            let mut chunks = response.bytes_stream();
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(|_| unavailable())?;
                if bytes.len().saturating_add(chunk.len()) > 1_048_576 {
                    return Err(unavailable());
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| unavailable())
        })
        .await
        .map_err(|_| unavailable())?
    }
}

fn require_lease(lease: &BridgeControlPlaneLease) -> Result<(), KeyIssuerError> {
    if tokio::time::Instant::now() >= lease.deadline() || lease.credential().trim().is_empty() {
        return Err(KeyIssuerError::UnexpectedAuthority);
    }
    if let ResourceOwner::Person { subject } = lease.owner() {
        if subject != lease.actor_subject() {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
    }
    Ok(())
}

fn attach_org(body: &mut serde_json::Value, owner: &ResourceOwner) {
    if let ResourceOwner::Organization { subject } = owner {
        body["target_org_id"] = serde_json::Value::String(subject.to_string());
    }
}

fn unavailable() -> KeyIssuerError {
    KeyIssuerError::Unavailable(
        "bridge Sandbox key operation unavailable or outcome unknown".into(),
    )
}
