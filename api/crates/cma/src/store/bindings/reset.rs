//! Incarnation heads and permanent reset receipts share the existing binding row.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, record_bson, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::binding::{
    reset_binding_eligibility, reset_eligibility, BindingError, ConversationHead,
    ConversationResetReceipt, ConversationResetRequest, ConversationResetState,
};
use layer2_core::sandbox::binding::{store::conversation_storage_key_for, ConversationResetFormat};
use mongodb::ClientSession;

use cma_am_bindings::reset::FIELD;
pub(super) use cma_am_bindings::reset::{read_state, read_state_for};

impl MongoSandboxStore {
    pub(super) async fn reset_conversation(
        &self,
        request: ConversationResetRequest,
        enabled: bool,
    ) -> Result<ConversationResetReceipt, BindingError> {
        request.validate()?;
        for _ in 0..8 {
            let mut session = self.client.start_session().await.map_err(driver)?;
            start_transaction(&mut session).await.map_err(driver)?;
            let result = self.apply_reset(&request, enabled, &mut session).await;
            match result {
                Ok(receipt) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => {
                        tracing::info!(binding_id = %request.binding_id, operation_id = %request.operation_id,
                            incarnation = receipt.head.incarnation, "binding conversation reset receipt confirmed");
                        return Ok(receipt);
                    }
                    _ => {
                        if let Some(receipt) = self.reset_receipt(&request).await? {
                            return Ok(receipt);
                        }
                    }
                },
                Err(error) => {
                    if let Err(abort) = session.abort_guarded().await {
                        tracing::debug!(error = %abort, "binding reset transaction abort failed; checking receipt");
                    }
                    if let Some(receipt) = self.reset_receipt(&request).await? {
                        return Ok(receipt);
                    }
                    if !matches!(error, BindingError::Conflict | BindingError::Unavailable(_)) {
                        return Err(error);
                    }
                }
            }
        }
        Err(BindingError::Conflict)
    }
    async fn reset_receipt(
        &self,
        request: &ConversationResetRequest,
    ) -> Result<Option<ConversationResetReceipt>, BindingError> {
        let binding = self
            .binding(&request.binding_id)
            .await?
            .ok_or(BindingError::NotFound)?;
        let scope = self
            .bindings
            .records
            .find_one(doc! {"_id": &request.idempotency_binding_id})
            .await
            .map_err(driver)?
            .ok_or(BindingError::NotFound)?;
        request.authorize(&binding, &read_binding(scope.clone())?)?;
        read_state(&scope)?.replay(request)
    }
    async fn apply_reset(
        &self,
        request: &ConversationResetRequest,
        enabled: bool,
        session: &mut ClientSession,
    ) -> Result<ConversationResetReceipt, BindingError> {
        let row = self
            .bindings
            .records
            .find_one(doc! {"_id": &request.binding_id})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .ok_or(BindingError::NotFound)?;
        let scope_row = if request.idempotency_binding_id == request.binding_id {
            row.clone()
        } else {
            self.bindings
                .records
                .find_one(doc! {"_id": &request.idempotency_binding_id})
                .session(&mut *session)
                .await
                .map_err(driver)?
                .ok_or(BindingError::NotFound)?
        };
        let binding = read_binding(row.clone())?;
        let scope = read_binding(scope_row.clone())?;
        request.authorize(&binding, &scope)?;
        let mut resets = read_state(&row)?;
        let mut receipts = read_state(&scope_row)?;
        if let Some(receipt) = receipts.replay(request)? {
            return Ok(receipt);
        }
        if !enabled {
            return Err(BindingError::ResetRefused("partition_reset_disabled"));
        }
        reset_binding_eligibility(&binding)?;
        let head = resets.head(binding.generation, &request.conversation_id);
        let internal = head.map_or(request.conversation_id.as_str(), |h| {
            h.successor.conversation_id.as_str()
        });
        let key = conversation_storage_key_for(&binding.binding_id, binding.generation, internal);
        let previous = self
            .bindings
            .conversations
            .find_one(doc! {"_id": key})
            .session(&mut *session)
            .await
            .map_err(driver)?
            .map(read_conversation)
            .transpose()?
            .ok_or(BindingError::ResetRefused(
                "partition_reset_mapping_missing",
            ))?;
        if previous.agent_id != request.expected_agent_id {
            return Err(BindingError::ResetRefused(
                "partition_reset_mapping_changed",
            ));
        }
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
            &binding,
            &previous,
            agent.as_ref(),
            sandbox.as_ref(),
            workspace.as_ref(),
        )?;
        let (Some(agent), Some(sandbox), Some(workspace)) = (agent, sandbox, workspace) else {
            return Err(BindingError::ResetRefused(
                "partition_reset_parents_unavailable",
            ));
        };
        crate::store::admission_checks::require_fresh(&self.budget_settings, session).await?;
        if (resets.heads.len() >= 256 && head.is_none()) || receipts.receipts.len() >= 512 {
            return Err(BindingError::ResetRefused("partition_reset_capacity"));
        }
        let incarnation = head
            .map_or(Some(1), |h| h.incarnation.checked_add(1))
            .ok_or(BindingError::Conflict)?;
        let internal = layer2_core::profiles::content_digest(&(
            &binding.binding_id,
            binding.generation,
            &request.conversation_id,
            incarnation,
        ))
        .map_err(|e| BindingError::Corrupt(e.to_string()))?;
        let successor = BindingConversation::new(
            &binding,
            format!("inc_{internal}"),
            layer2_core::now_rfc3339(),
        )
        .map_err(BindingError::Corrupt)?;
        let head = ConversationHead {
            transport_conversation_id: request.conversation_id.clone(),
            incarnation,
            successor,
        };
        let receipt = ConversationResetReceipt {
            request: request.clone(),
            previous,
            head: head.clone(),
        };
        resets.heads.insert(
            ConversationResetState::key(binding.generation, &request.conversation_id),
            head,
        );
        if request.idempotency_binding_id == request.binding_id {
            resets
                .receipts
                .insert(request.operation_id.clone(), receipt.clone());
        } else {
            receipts
                .receipts
                .insert(request.operation_id.clone(), receipt.clone());
            self.write_reset_state(&scope, &receipts, session).await?;
        }
        // Write the parent revisions as well as reading them: concurrent deletion,
        // admission and lifecycle changes conflict with this transaction.
        for (collection, id, revision) in [
            (&self.agents, agent.id.as_str(), agent.revision),
            (&self.sandboxes, sandbox.id.as_str(), sandbox.revision),
            (&self.workspaces, workspace.id.as_str(), workspace.revision),
        ] {
            let result = collection
                .update_one(
                    doc! {"_id": id, "revision": integer(revision)?},
                    doc! {"$inc":{"binding_reset_fence":1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(driver)?;
            if result.matched_count != 1 {
                return Err(BindingError::Conflict);
            }
        }
        self.write_reset_state(&binding, &resets, session).await?;
        Ok(receipt)
    }
    async fn write_reset_state(
        &self,
        binding: &ProviderBinding,
        resets: &ConversationResetState,
        session: &mut ClientSession,
    ) -> Result<(), BindingError> {
        if !resets
            .within_capacity()
            .map_err(|error| BindingError::Corrupt(error.to_string()))?
        {
            return Err(BindingError::ResetRefused("partition_reset_capacity"));
        }
        let mut next = binding.clone();
        next.reset_format = Some(ConversationResetFormat::V1);
        next.advance_revision(layer2_core::now_rfc3339())
            .map_err(BindingError::Corrupt)?;
        ConversationResetState::validate_format(&next, Some(resets))
            .map_err(BindingError::Corrupt)?;
        let mut set = binding_document(&next)?;
        set.remove("_id");
        set.insert(FIELD, record_bson(resets).map_err(BindingError::Corrupt)?);
        if mongodb::bson::serialize_to_vec(&set)
            .map_err(|error| BindingError::Corrupt(error.to_string()))?
            .len()
            > 8 * 1024 * 1024
        {
            return Err(BindingError::ResetRefused("partition_reset_capacity"));
        }
        let result = self
            .bindings
            .records
            .update_one(
                doc! {"_id":&binding.binding_id,"revision":integer(binding.revision)?, "document_schema_version": DOCUMENT_SCHEMA_VERSION},
                doc! {"$set":set,"$inc":{"admission_fence":1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(BindingError::Conflict);
        }
        Ok(())
    }
}
