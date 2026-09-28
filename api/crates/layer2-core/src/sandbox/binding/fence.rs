use super::{
    BindingConversation, BindingConversationState, BindingState, BindingWork, ProviderBinding,
};
use crate::sandbox::store::{AgentAdmission, SandboxStoreError};
use crate::sandbox::{Sandbox, Workspace};

/// A persisted parent reservation must still authorize a resource insertion in
/// the same transaction. A process-local worker claim cannot supply this proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingAdmissionFence {
    pub binding_id: String,
    pub revision: u64,
    pub generation: u64,
    pub conversation: Option<BindingConversationFence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingConversationFence {
    pub conversation_id: String,
    pub revision: u64,
}

impl BindingAdmissionFence {
    pub fn environment(binding: &ProviderBinding) -> Self {
        Self {
            binding_id: binding.binding_id.clone(),
            revision: binding.revision,
            generation: binding.generation,
            conversation: None,
        }
    }

    pub fn conversation(binding: &ProviderBinding, conversation: &BindingConversation) -> Self {
        Self {
            conversation: Some(BindingConversationFence {
                conversation_id: conversation.conversation_id.clone(),
                revision: conversation.revision,
            }),
            ..Self::environment(binding)
        }
    }

    fn require_binding(&self, binding: &ProviderBinding) -> Result<(), SandboxStoreError> {
        binding.validate().map_err(SandboxStoreError::Corrupt)?;
        if self.binding_id != binding.binding_id
            || binding.existing_agent.is_some()
            || self.revision != binding.revision
            || self.generation != binding.generation
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    pub fn require_sandbox(
        &self,
        binding: &ProviderBinding,
        sandbox: &Sandbox,
    ) -> Result<(), SandboxStoreError> {
        self.require_binding(binding)?;
        let profile = binding
            .profile
            .environment
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if self.conversation.is_some()
            || binding.state != BindingState::Provisioning
            || binding.work != BindingWork::ProvisionSandbox
            || sandbox.id != binding.sandbox_id
            || sandbox.owner != binding.owner
            || sandbox.profile.content != profile.sandbox
            || sandbox.credential.source != binding.credential_source()
            || sandbox.credential.reaches != profile.sandbox.nyxid_services
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    pub fn require_workspace(
        &self,
        binding: &ProviderBinding,
        workspace: &Workspace,
    ) -> Result<(), SandboxStoreError> {
        self.require_binding(binding)?;
        let profile = binding
            .profile
            .environment
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if self.conversation.is_some()
            || binding.state != BindingState::Provisioning
            || binding.work != BindingWork::ProvisionWorkspace
            || workspace.id != binding.workspace_id
            || workspace.sandbox_id != binding.sandbox_id
            || workspace.owner != binding.owner
            || workspace.profile.content != profile.workspace
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    pub fn require_agent(
        &self,
        binding: &ProviderBinding,
        conversation: &BindingConversation,
        admission: &AgentAdmission,
    ) -> Result<(), SandboxStoreError> {
        self.require_binding(binding)?;
        conversation
            .validate()
            .map_err(SandboxStoreError::Corrupt)?;
        let proof = self
            .conversation
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if binding.state != BindingState::Ready
            || binding.work != BindingWork::Idle
            || conversation.binding_id != binding.binding_id
            || conversation.generation != binding.generation
            || proof.conversation_id != conversation.conversation_id
            || proof.revision != conversation.revision
            || conversation.state != BindingConversationState::Provisioning
            || conversation.owner != binding.owner
            || conversation.sandbox_id != binding.sandbox_id
            || conversation.workspace_id != binding.workspace_id
            || admission.owner != binding.owner
            || admission.workspace_id != binding.workspace_id
            || admission.agent_id != conversation.agent_id
            || admission.worktree_id != conversation.worktree_id
            || admission.profile.content != binding.profile.agent
            || admission.creation_key != conversation.creation_key()?
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

impl BindingConversation {
    pub fn creation_key(&self) -> Result<String, SandboxStoreError> {
        crate::profiles::content_digest(&(&self.binding_id, self.generation, &self.conversation_id))
            .map(|hash| format!("binding:{hash}"))
            .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))
    }
}
