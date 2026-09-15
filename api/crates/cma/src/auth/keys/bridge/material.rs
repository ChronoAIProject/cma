use super::{BridgeSandboxKeyPlan, KeyIssuer, KeyIssuerError, MintedKey};
use crate::bridge::BridgeControlPlaneLease;

impl KeyIssuer {
    /// Revalidate the exact current scopes and owner before runtime delivery.
    pub(crate) async fn require_bridge_sandbox_material(
        &self,
        lease: &BridgeControlPlaneLease,
        plan: &BridgeSandboxKeyPlan,
        material: &MintedKey,
    ) -> Result<(), KeyIssuerError> {
        super::require_lease(lease)?;
        if plan.owner != *lease.owner()
            || plan.actor != *lease.actor_subject()
            || plan.bridge_id != *lease.bridge_id()
            || plan.connector != *lease.connector()
            || plan.grant != *lease.grant()
            || tokio::time::Instant::now() >= plan.deadline
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let metadata: super::contract::KeyMetadata = self
            .bridge_json(
                self.http.get(format!(
                    "{}{}/{}",
                    self.base_url,
                    super::super::KEYS_PATH,
                    material.id
                )),
                lease,
            )
            .await?;
        metadata.require(
            &plan.owner,
            &plan.actor,
            &super::super::key_name(&self.deployment, &plan.credential_id)?,
            material,
            &plan.scope,
        )?;
        super::require_lease(lease)
    }
}
