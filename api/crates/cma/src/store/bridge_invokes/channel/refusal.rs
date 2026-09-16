use layer2_core::{
    bridge::invoke::{BridgeInvokeGrant, BridgeInvokeGrantState},
    sandbox::agent_io::{
        AgentActor, ChannelCommandAdmissionError as Error, ChannelHoldRefusal,
        ChannelHoldRefusalReason,
    },
};
use mongodb::{bson::doc, ClientSession};
use std::sync::Arc;

use super::{MongoChannelCommandStore, MutationError};
use crate::store::{
    agent_commands::command_document,
    sandbox_mongo::{
        commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
    },
};

impl MongoChannelCommandStore {
    pub(super) async fn refuse_held(&self, refusal: Arc<ChannelHoldRefusal>) -> Result<(), Error> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            refusal.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            if let Err(error) = self.refuse_transaction(&refusal, &mut session).await {
                let _ = session.abort_transaction().await;
                match error {
                    MutationError::Driver(error) if is_transient(&error) => continue,
                    MutationError::Driver(_) => return Err(Error::Unavailable),
                    MutationError::Core(error) => return Err(error),
                }
            }
            if let Err(error) = refusal.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(Error::Unavailable),
            }
        }
        Err(Error::Unavailable)
    }

    async fn refuse_transaction(
        &self,
        refusal: &ChannelHoldRefusal,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        refusal.require_current()?;
        let command = refusal.command();
        let source = self.invokes.read_channel_source(command, session).await?;
        if &source != refusal.source() {
            return Err(Error::SourceMismatch.into());
        }
        super::super::channel_source::fence(
            &self.invokes,
            command,
            &source.sidecar,
            &source.inbox,
            session,
        )
        .await?;
        let allocation_fence = self
            .allocations
            .update_one(
                doc! {"_id": &source.allocation.id, "revision": source.allocation.revision as i64},
                doc! {"$inc": {"held_refusal_fence": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if allocation_fence.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
        self.require_missing_source(refusal, session).await?;
        let revoked = if refusal.reason() == ChannelHoldRefusalReason::GrantRevoked {
            let AgentActor::Channel { grant_id, .. } = &command.receipt.envelope.actor else {
                return Err(Error::SourceMismatch.into());
            };
            let document = self
                .invokes
                .grants
                .find_one(doc! {"_id": grant_id})
                .session(&mut *session)
                .await?
                .ok_or(Error::SourceMismatch)?;
            let grant: BridgeInvokeGrant =
                super::super::read_grant(&document).map_err(Error::from)?;
            if !matches!(grant.state, BridgeInvokeGrantState::Revoked { .. })
                || grant.pin != source.inbox.pin
                || grant.owner != command.receipt.envelope.owner
                || grant.target.ids != *source.allocation.placement.target()
                || command.receipt.envelope.authorization.authority_id
                    != format!("bridge_invoke:{}", grant.id.as_str())
            {
                return Err(Error::SourceMismatch.into());
            }
            let changed = self
                .invokes
                .grants
                .update_one(
                    doc! {"_id": grant.id.as_str(), "revision": grant.revision as i64},
                    doc! {"$inc": {"held_refusal_fence": 1_i64}},
                )
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(Error::SourceMismatch.into());
            }
            Some(grant)
        } else {
            None
        };
        let next = refusal.refused_command()?;
        let mut fields = command_document(&next).map_err(Error::from)?;
        fields.remove("_id");
        fields.insert(
            "channel_execution_refusal",
            doc! {
                "reason": match refusal.reason() {
                    ChannelHoldRefusalReason::QueueTimeout => "queue_timeout",
                    ChannelHoldRefusalReason::GrantRevoked => "grant_revoked",
                    ChannelHoldRefusalReason::TargetMissing => "target_missing",
                    ChannelHoldRefusalReason::BridgeMissing => "bridge_missing",
                    ChannelHoldRefusalReason::GrantMissing => "grant_missing",
                },
                "queue_deadline": &command.queue_deadline,
                "grant_id": revoked.as_ref().map(|grant| grant.id.as_str()),
                "grant_revision": revoked.as_ref().map(|grant| grant.revision as i64),
                "at": &next.work.failure.as_ref().ok_or(Error::SourceMismatch)?.at,
            },
        );
        let changed = self.invokes.resources.agent_commands.update_one(
            doc! {"_id": command.receipt.envelope.command_id.as_str(), "revision": command.revision as i64, "status": "waiting_action"},
            doc! {"$set": fields},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
        Ok(())
    }

    async fn require_missing_source(
        &self,
        refusal: &ChannelHoldRefusal,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let target = refusal.source().allocation.placement.target();
        let missing = match refusal.reason() {
            ChannelHoldRefusalReason::TargetMissing => {
                let resources = &self.invokes.resources;
                let mut missing = false;
                for (collection, id) in [
                    (&resources.sandboxes, target.sandbox_id.as_str()),
                    (&resources.workspaces, target.workspace_id.as_str()),
                    (&resources.worktrees, target.worktree_id.as_str()),
                    (&resources.agents, target.agent_id.as_str()),
                ] {
                    if collection
                        .find_one(doc! {"_id": id})
                        .session(&mut *session)
                        .await?
                        .is_none()
                    {
                        missing = true;
                        break;
                    }
                }
                missing
            }
            ChannelHoldRefusalReason::BridgeMissing => self
                .invokes
                .bridges
                .definitions
                .find_one(doc! {"_id": refusal.source().inbox.pin.bridge_id.as_str()})
                .session(&mut *session)
                .await?
                .is_none(),
            ChannelHoldRefusalReason::GrantMissing => {
                let AgentActor::Channel { grant_id, .. } =
                    &refusal.command().receipt.envelope.actor
                else {
                    return Err(Error::SourceMismatch.into());
                };
                self.invokes
                    .grants
                    .find_one(doc! {"_id": grant_id})
                    .session(&mut *session)
                    .await?
                    .is_none()
            }
            ChannelHoldRefusalReason::QueueTimeout | ChannelHoldRefusalReason::GrantRevoked => {
                return Ok(())
            }
        };
        if !missing {
            return Err(Error::SourceMismatch.into());
        }
        Ok(())
    }
}
