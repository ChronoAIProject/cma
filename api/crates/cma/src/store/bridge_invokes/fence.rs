//! Current invoke authority shared by admission and exact retained execution.
use super::*;

impl MongoBridgeInvokeStore {
    pub(super) async fn fence_transaction(
        &self,
        proof: &BridgeInvokeProof,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.fence_observed_transaction(proof, None, session).await
    }

    pub(super) async fn fence_observed_transaction(
        &self,
        proof: &BridgeInvokeProof,
        retained: Option<&layer2_core::bridge::allocation::AllocationRecord>,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        proof.require_current()?;
        let bridge = self.read_bridge(&proof.bridge().id, session).await?;
        let grant = self
            .grants
            .find_one(doc! {"_id":proof.grant().id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(read_grant)
            .transpose()?
            .ok_or(BridgeInvokeError::NotFound)?;
        grant.require_active()?;
        let resources =
            read_execution_snapshot(&self.resources, &grant.target.resource(), session).await?;
        proof.require_snapshot(&bridge, &grant, &resources)?;
        if let Some(basis) = &grant.channel {
            basis.require_bridge(&bridge, &grant.pin, &grant.config)?;
            match retained {
                Some(allocation)
                    if matches!(proof.effect(), BridgeInvokeEffect::ExecuteCommand { .. }) =>
                {
                    self.require_retained_channel_grant(basis, allocation, true, session)
                        .await?;
                }
                Some(_) => return Err(BridgeInvokeError::TargetMismatch.into()),
                None => self.fence_channel_grant(basis, session).await?,
            }
        }
        if grant.channel.is_none() {
            if let Some(allocation) = retained {
                if !matches!(proof.effect(), BridgeInvokeEffect::ExecuteCommand { .. })
                    || allocation.pin != grant.pin
                    || allocation.config != grant.config
                    || allocation.owner != grant.owner
                    || !matches!(&allocation.placement,
                        layer2_core::bridge::allocation::AllocationPlacement::ExistingAgent { target, grant_id }
                        if target == &grant.target.ids && grant_id == grant.id.as_str())
                {
                    return Err(BridgeInvokeError::TargetMismatch.into());
                }
                if allocation.state == layer2_core::bridge::allocation::AllocationState::Retired {
                    self.require_retained_fixed(allocation, true, session)
                        .await?;
                }
            }
        }
        self.bridges
            .credentials
            .require_retained_binding(&bridge, &grant.config, proof.custody_pin(), session)
            .await?;
        self.fence_bridge(&bridge, session).await?;
        let changed = self
            .grants
            .update_one(
                doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64},
                doc! {"$inc":{"effect_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        fence_lineage(&self.resources, &resources, session).await?;
        proof.require_current()?;
        Ok(())
    }
}
