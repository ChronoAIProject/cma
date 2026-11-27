//! Atomically move a deleted conversation to a distinct immutable profile binding.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::work_claim_authority::without_ledger_maintenance;
use layer2_core::sandbox::binding::{
    reset_binding_eligibility, reset_eligibility, store::conversation_storage_key_for,
    BindingConversationState, BindingError, ConversationResetState, ProfileTransferReceipt,
    ProfileTransferRequest,
};
use mongodb::ClientSession;

impl MongoSandboxStore {
    pub(super) async fn transfer_conversation(
        &self,
        request: ProfileTransferRequest,
        enabled: bool,
    ) -> Result<ProfileTransferReceipt, BindingError> {
        request.validate()?;
        self.bindings.safety.require_associations()?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self.client.start_session().await.map_err(driver)?;
            start_transaction(&mut session).await.map_err(driver)?;
            let receipt = match self.apply_transfer(&request, enabled, &mut session).await {
                Ok(receipt) => receipt,
                Err(error) => {
                    let aborted = session.abort_guarded().await.is_ok();
                    if aborted && matches!(error, BindingError::Conflict) {
                        continue;
                    }
                    return Err(error);
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt),
                CommitOutcome::Retry(_) => {
                    let _ = session.abort_guarded().await;
                    continue;
                }
                CommitOutcome::Unknown(error) => {
                    // Observe only the exact durable receipt. An uncertain commit
                    // cannot authorize another transaction in this invocation.
                    if let Some(stored) = self.transfer_readback(&receipt).await? {
                        return Ok(stored);
                    }
                    return Err(BindingError::Unavailable(error.to_string()));
                }
                CommitOutcome::Failed(error) => return Err(driver(error).into()),
            }
        }
        Err(BindingError::Conflict)
    }

    async fn transfer_readback(
        &self,
        receipt: &ProfileTransferReceipt,
    ) -> Result<Option<ProfileTransferReceipt>, BindingError> {
        let request = &receipt.request;
        let source = self
            .binding_conversation(
                &request.source_binding_id,
                request.expected_generation,
                &receipt.source_conversation_id,
            )
            .await?;
        Ok(source.and_then(|source| {
            source
                .profile_transfer
                .and_then(|stored| (*stored == *receipt).then_some(*stored))
        }))
    }

    async fn apply_transfer(
        &self,
        request: &ProfileTransferRequest,
        enabled: bool,
        session: &mut ClientSession,
    ) -> Result<ProfileTransferReceipt, BindingError> {
        let (source, resets) = self
            .transfer_binding(&request.source_binding_id, session)
            .await?;
        let (destination, destination_resets) = self
            .transfer_binding(&request.destination_binding_id, session)
            .await?;
        request.authorize(&source, &destination)?;
        let head = resets.head(request.expected_generation, &request.conversation_id);
        let internal = head.map_or(request.conversation_id.as_str(), |head| {
            head.successor.conversation_id.as_str()
        });
        let key =
            conversation_storage_key_for(&source.binding_id, request.expected_generation, internal);
        let mut previous = self
            .bindings
            .conversations
            .find_one(doc! {"_id": &key})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .map(read_conversation)
            .transpose()?
            .ok_or(BindingError::ResetRefused(
                "partition_reset_mapping_missing",
            ))?;
        if let Some(receipt) = previous.profile_transfer.as_ref() {
            return if receipt.request == *request {
                Ok((**receipt).clone())
            } else {
                Err(BindingError::ResetRefused("partition_reset_key_conflict"))
            };
        }
        if !enabled {
            return Err(BindingError::ResetRefused("partition_reset_disabled"));
        }
        if source.generation != request.expected_generation
            || destination.generation != request.expected_destination_generation
            || previous.agent_id != request.expected_agent_id
            || head.map_or(0, |head| head.incarnation) != request.expected_incarnation
        {
            return Err(BindingError::ResetRefused(
                "partition_reset_mapping_changed",
            ));
        }
        reset_binding_eligibility(&destination)?;
        let destination_key = conversation_storage_key_for(
            &destination.binding_id,
            destination.generation,
            &request.conversation_id,
        );
        if destination_resets
            .head(destination.generation, &request.conversation_id)
            .is_some()
            || self
                .bindings
                .conversations
                .find_one(doc! {"_id": &destination_key})
                .session(&mut *session)
                .await
                .map_err(driver)?
                .is_some()
        {
            return Err(BindingError::ResetRefused(
                "partition_reset_destination_exists",
            ));
        }
        self.fence_transfer_parents(&source, &previous, session)
            .await?;
        crate::store::admission_checks::require_fresh(&self.budget_settings, session).await?;
        for binding in [&source, &destination] {
            let result = self
                .bindings
                .records
                .update_one(
                    doc! {"_id": &binding.binding_id, "revision": integer(binding.revision)?,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION},
                    doc! {"$inc": {"admission_fence":1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(driver)?;
            if result.matched_count != 1 {
                return Err(BindingError::Conflict);
            }
        }
        let at = layer2_core::now_rfc3339();
        let successor =
            BindingConversation::new(&destination, request.conversation_id.clone(), at.clone())
                .map_err(BindingError::Corrupt)?;
        let receipt = ProfileTransferReceipt {
            request: request.clone(),
            source_conversation_id: previous.conversation_id.clone(),
            destination_generation: successor.generation,
            destination_agent_id: successor.agent_id.clone(),
            destination_worktree_id: successor.worktree_id.clone(),
        };
        let revision = previous.revision;
        previous.revision = revision.checked_add(1).ok_or(BindingError::Conflict)?;
        previous.updated_at = at;
        previous.state = BindingConversationState::Retired;
        previous.profile_transfer = Some(Box::new(receipt.clone()));
        let result = self
            .bindings
            .conversations
            .replace_one(
                doc! {"_id": key, "revision": integer(revision)?},
                conversation_document(&previous)?,
            )
            .session(&mut *session)
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(BindingError::Conflict);
        }
        self.bindings
            .conversations
            .insert_one(conversation_document(&successor)?)
            .session(&mut *session)
            .await
            .map_err(driver)?;
        Ok(receipt)
    }

    async fn transfer_binding(
        &self,
        id: &str,
        session: &mut ClientSession,
    ) -> Result<(ProviderBinding, ConversationResetState), BindingError> {
        let row = self
            .bindings
            .records
            .find_one(doc! {"_id": id})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .ok_or(BindingError::NotFound)?;
        let binding = binding_record(&row)?;
        let resets = reset::read_state_for(&binding, &row)?;
        Ok((binding, resets))
    }

    async fn fence_transfer_parents(
        &self,
        binding: &ProviderBinding,
        previous: &BindingConversation,
        session: &mut ClientSession,
    ) -> Result<(), BindingError> {
        let agent = self
            .agents
            .find_one(doc! {"_id": previous.agent_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .as_ref()
            .map(crate::store::agents::read_agent)
            .transpose()?;
        let sandbox = self
            .sandboxes
            .find_one(doc! {"_id": binding.sandbox_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .as_ref()
            .map(crate::store::sandboxes::read_sandbox)
            .transpose()?;
        let workspace = self
            .workspaces
            .find_one(doc! {"_id": binding.workspace_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .as_ref()
            .map(crate::store::workspaces::read_workspace)
            .transpose()?;
        reset_eligibility(
            binding,
            previous,
            agent.as_ref(),
            sandbox.as_ref(),
            workspace.as_ref(),
        )?;
        let (Some(agent), Some(sandbox), Some(workspace)) = (agent, sandbox, workspace) else {
            return Err(BindingError::ResetRefused(
                "partition_reset_parents_unavailable",
            ));
        };
        for (collection, id, revision) in [
            (&self.agents, agent.id.as_str(), agent.revision),
            (&self.sandboxes, sandbox.id.as_str(), sandbox.revision),
            (&self.workspaces, workspace.id.as_str(), workspace.revision),
        ] {
            let result = collection
                .update_one(
                    without_ledger_maintenance(doc! {"_id": id, "revision": integer(revision)?}),
                    doc! {"$inc": {"binding_reset_fence": 1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(driver)?;
            if result.matched_count != 1 {
                return Err(BindingError::Conflict);
            }
        }
        Ok(())
    }
}
