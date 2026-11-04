use super::*;
use layer2_core::sandbox::binding::BindingConversationAuthority;
use mongodb::ClientSession;

pub(in crate::store) enum FenceError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}
impl From<SandboxStoreError> for FenceError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for FenceError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl BindingCollections {
    /// Requeue bookkeeping only: current binding/conversation must still own this command.
    pub(in crate::store) async fn continuation_retry_authority(
        &self,
        envelope: &layer2_core::sandbox::agent_io::AgentCommandEnvelope,
        session: &mut ClientSession,
    ) -> Result<bool, FenceError> {
        let Some(id) = envelope.authorization.authority_id.strip_prefix("binding:") else {
            return Ok(false);
        };
        let Some(binding) = self
            .records
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .map(read_binding)
            .transpose()?
        else {
            return Ok(false);
        };
        let Some(conversation) = self.conversations.find_one(doc! {"binding_id":id,"generation":integer(binding.generation)?,"record.agent_id":envelope.agent_id.as_str()}).session(&mut *session).await?.map(read_conversation).transpose()? else { return Ok(false); };
        let proof = BindingConversationAuthority {
            binding,
            conversation,
        };
        if proof.require_command(envelope).is_err() {
            return Ok(false);
        }
        self.conversation_authority(Some(&proof), session).await?;
        Ok(true)
    }

    pub(in crate::store) async fn conversation_authority(
        &self,
        proof: Option<&BindingConversationAuthority>,
        session: &mut ClientSession,
    ) -> Result<(), FenceError> {
        let Some(proof) = proof else { return Ok(()) };
        // Conversation CAS, retirement and key replacement all write this
        // parent, making the observation serializable with the command effect.
        let result = self
            .records
            .update_one(
                doc! { "_id": &proof.binding.binding_id,
                "revision": integer(proof.binding.revision)?,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION },
                doc! { "$inc": { "admission_fence": 1_i64 } },
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
        let binding = self
            .records
            .find_one(doc! {"_id": &proof.binding.binding_id})
            .session(&mut *session)
            .await?
            .map(read_binding)
            .transpose()?
            .ok_or(SandboxStoreError::Conflict)?;
        let key =
            layer2_core::sandbox::binding::store::conversation_storage_key(&proof.conversation);
        let conversation = self
            .conversations
            .find_one(doc! {"_id": key})
            .session(&mut *session)
            .await?
            .map(read_conversation)
            .transpose()?
            .ok_or(SandboxStoreError::Conflict)?;
        proof.require_current(&binding, &conversation)?;
        Ok(())
    }
}

impl From<FenceError> for crate::store::agent_commands::ApplyError {
    fn from(error: FenceError) -> Self {
        use layer2_core::sandbox::agent_io::CommandLedgerError;
        match error {
            FenceError::Driver(error) => Self::Driver(error),
            FenceError::Domain(SandboxStoreError::Corrupt(_)) => {
                Self::Domain(CommandLedgerError::Corrupt)
            }
            FenceError::Domain(_) => Self::Domain(CommandLedgerError::Conflict),
        }
    }
}
impl From<FenceError> for crate::store::agent_conversation_store::ApplyError {
    fn from(error: FenceError) -> Self {
        match error {
            FenceError::Driver(error) => Self::Driver(error),
            FenceError::Domain(error) => Self::Domain(error),
        }
    }
}
impl From<FenceError> for crate::store::sandbox_transactions::ApplyError {
    fn from(error: FenceError) -> Self {
        match error {
            FenceError::Driver(error) => Self::Driver(error),
            FenceError::Domain(error) => Self::Domain(error),
        }
    }
}
