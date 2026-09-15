use layer2_core::bridge::allocation::AllocationPlacement;
use layer2_core::sandbox::agent_io::AgentActor;

use super::*;

impl MongoBridgeReplyToolStore {
    pub(super) async fn require_turn_grant(
        &self,
        observed: &TurnSnapshot,
        write_fence: bool,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let AgentActor::Channel { grant_id, .. } = &observed.command.receipt.envelope.actor else {
            return Err(OutboxError::SourceMismatch.into());
        };
        let document = self
            .invokes
            .grants
            .find_one(doc! {"_id": grant_id})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::AuthorityUnavailable)?;
        let grant = super::super::super::read_grant(&document)
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        grant
            .require_active()
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        if grant.pin != observed.inbox.pin
            || grant.owner != observed.inbox.owner
            || grant.target.ids != *observed.allocation.placement.target()
            || grant.config
                != observed
                    .bridge
                    .pinned(&observed.inbox.pin)
                    .map_err(|_| OutboxError::SourceMismatch)?
                    .config
            || observed.command.receipt.envelope.authorization.authority_id
                != format!("bridge_invoke:{}", grant.id.as_str())
        {
            return Err(OutboxError::SourceMismatch.into());
        }
        grant
            .target
            .require(&grant.owner, &observed.resources.resource)
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        match &observed.allocation.placement {
            AllocationPlacement::ExistingAgent {
                grant_id: original, ..
            } => {
                if grant_id != original || grant.channel.is_some() {
                    return Err(OutboxError::SourceMismatch.into());
                }
            }
            AllocationPlacement::NewSandbox { .. } => {
                let basis = grant.channel.as_ref().ok_or(OutboxError::SourceMismatch)?;
                if basis.allocation != observed.allocation || basis.creation != observed.provenance
                {
                    return Err(OutboxError::SourceMismatch.into());
                }
                basis
                    .require_bridge(&observed.bridge, &grant.pin, &grant.config)
                    .map_err(|_| OutboxError::AuthorityUnavailable)?;
                basis
                    .require_resources(&observed.resources.resource)
                    .map_err(|_| OutboxError::AuthorityUnavailable)?;
                if write_fence {
                    self.invokes
                        .fence_channel_grant(basis, session)
                        .await
                        .map_err(|_| OutboxError::AuthorityUnavailable)?;
                }
            }
        }
        if write_fence {
            let changed = self
                .invokes
                .grants
                .update_one(
                    doc! {"_id": grant.id.as_str(), "revision": grant.revision as i64},
                    doc! {"$inc": {"outbox_fence": 1_i64}},
                )
                .session(session)
                .await?;
            if changed.matched_count != 1 {
                return Err(OutboxError::Conflict.into());
            }
        }
        Ok(())
    }
}
