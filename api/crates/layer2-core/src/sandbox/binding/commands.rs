use super::{
    BindingConversation, BindingConversationAuthority, BindingConversationState, BindingPrincipal,
    BindingState, BindingWork, ProviderBinding,
};
use crate::sandbox::agent_io::{
    AgentActor, AgentAuthorizationRevision, AgentCommandEnvelope, AgentSourceKind,
    CommandLedgerError, CommandReservation,
};
use crate::sandbox::store::SandboxStoreError;

impl BindingPrincipal {
    pub fn command_authority(&self) -> AgentAuthorizationRevision {
        authority(self.binding_id(), self.generation())
    }
}

impl BindingConversationAuthority {
    pub fn command_authority(&self) -> AgentAuthorizationRevision {
        authority(&self.binding.binding_id, self.binding.generation)
    }
    /// The store supplies both records from its locked state/transaction. The
    /// parent write serializes this observation with all conversation changes.
    pub fn require_current(
        &self,
        binding: &ProviderBinding,
        conversation: &BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        binding.validate().map_err(SandboxStoreError::Corrupt)?;
        conversation
            .validate()
            .map_err(SandboxStoreError::Corrupt)?;
        if binding != &self.binding
            || conversation != &self.conversation
            || !(binding.state == BindingState::Ready
                || binding.state == BindingState::Provisioning
                    && binding.work == BindingWork::DeliverCredential)
            || conversation.state != BindingConversationState::Ready
            || conversation.binding_id != binding.binding_id
            || conversation.generation != binding.generation
            || conversation.owner != binding.owner
            || conversation.sandbox_id != binding.sandbox_id
            || conversation.workspace_id != binding.workspace_id
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    pub fn require_command(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<(), CommandLedgerError> {
        self.require_current(&self.binding, &self.conversation)
            .map_err(|_| CommandLedgerError::Conflict)?;
        if envelope.command.external_event().is_some_and(|context| {
            context
                .provider_conversation_id
                .as_deref()
                .unwrap_or(&context.conversation_id)
                != self.conversation.conversation_id
        }) {
            return Err(CommandLedgerError::Invalid);
        }
        if envelope.authorization != authority(&self.binding.binding_id, self.binding.generation)
            || envelope.agent_id != self.conversation.agent_id
            || envelope.owner != self.binding.resource_owner()
            || envelope.actor
                != (AgentActor::Api {
                    subject: self.binding.owner.clone(),
                    delegation_id: None,
                })
            || envelope.source.kind != AgentSourceKind::Api
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }
}

impl CommandReservation {
    pub(crate) fn validate_binding(&self) -> Result<(), CommandLedgerError> {
        match &self.binding {
            Some(proof) => proof.require_command(&self.envelope),
            None if self
                .envelope
                .authorization
                .authority_id
                .starts_with("binding:") =>
            {
                Err(CommandLedgerError::Invalid)
            }
            None if self.envelope.command.external_event().is_some() => {
                Err(CommandLedgerError::Invalid)
            }
            None => Ok(()),
        }
    }
}

fn authority(id: &str, generation: u64) -> AgentAuthorizationRevision {
    AgentAuthorizationRevision {
        authority_id: format!("binding:{id}"),
        policy_revision: 1,
        grant_revision: Some(generation),
    }
}
