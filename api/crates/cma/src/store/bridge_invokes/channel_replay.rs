use super::*;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
use layer2_core::bridge::BridgeManagementLease;

impl MongoBridgeInvokeStore {
    pub(super) async fn channel_replay(
        &self,
        grant: &BridgeInvokeGrant,
        management: &BridgeManagementLease,
        control_plane: &dyn ChannelInvokeAuthority,
    ) -> Result<(), BridgeInvokeError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            let result = self
                .channel_replay_in(grant, management, control_plane, &mut session)
                .await;
            match result {
                Ok(()) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(()),
                    CommitOutcome::Retry(_) => continue,
                    _ => return Err(BridgeInvokeError::Unavailable),
                },
                Err(MutationError::Mongo(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                }
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    return Err(error.into_invoke());
                }
            }
        }
        Err(BridgeInvokeError::Unavailable)
    }

    async fn channel_replay_in(
        &self,
        grant: &BridgeInvokeGrant,
        management: &BridgeManagementLease,
        control_plane: &dyn ChannelInvokeAuthority,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        management.require_current()?;
        control_plane.require_current()?;
        let bridge = self.read_bridge(&grant.pin.bridge_id, session).await?;
        if &bridge != management.record() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let basis = grant.channel.as_ref().ok_or(BridgeInvokeError::Corrupt)?;
        basis.require_bridge(&bridge, &grant.pin, &grant.config)?;
        let current = self
            .grants
            .find_one(doc! {"_id":grant.id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(read_grant)
            .transpose()?
            .ok_or(BridgeInvokeError::NotFound)?;
        if &current != grant {
            return Err(BridgeInvokeError::Conflict.into());
        }
        self.bridges
            .credentials
            .require_channel_invoke_binding(
                &bridge,
                &basis.allocation.config.connector,
                control_plane,
                session,
            )
            .await
            .map_err(|error| match error {
                super::super::bridge_credentials::control_plane::ControlPlaneFenceError::Core(
                    _,
                ) => MutationError::Invoke(BridgeInvokeError::Unavailable),
                super::super::bridge_credentials::control_plane::ControlPlaneFenceError::Driver(
                    error,
                ) => MutationError::Mongo(error),
            })?;
        self.fence_channel_grant(basis, session).await?;
        let resources =
            read_execution_snapshot(&self.resources, &grant.target.resource(), session).await?;
        basis.require_resources(&resources)?;
        fence_lineage(&self.resources, &resources, session).await?;
        self.fence_bridge(&bridge, session).await?;
        let changed = self
            .grants
            .update_one(
                doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64},
                doc! {"$inc":{"replay_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        management.require_current()?;
        control_plane.require_current()?;
        Ok(())
    }
}
