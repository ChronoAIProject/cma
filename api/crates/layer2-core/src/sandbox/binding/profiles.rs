//! Binding records already own the immutable plan; do not create catalogue drafts.
use super::ProviderBinding;
use crate::profiles::*;
use crate::{AgentProfileId, SandboxProfileId, WorkspaceProfileId};

pub(super) struct BindingProfiles;
impl BindingProfiles {
    pub fn agent(
        &self,
        binding: &ProviderBinding,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        let content = binding.profile.agent.clone();
        Ok(AgentProfileRevision {
            profile_id: AgentProfileId(format!("agp_{}", identity(binding, &content)?)),
            revision: 1,
            digest: content_digest(&content)?,
            content,
            published_at: binding.created_at.clone(),
        })
    }
    pub fn workspace(
        &self,
        binding: &ProviderBinding,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        let content = binding
            .profile
            .environment
            .as_ref()
            .ok_or(ProfileStoreError::NotFound)?
            .workspace
            .clone();
        Ok(WorkspaceProfileRevision {
            profile_id: WorkspaceProfileId(format!("wkp_{}", identity(binding, &content)?)),
            revision: 1,
            digest: content_digest(&content)?,
            content,
            published_at: binding.created_at.clone(),
        })
    }
    pub fn sandbox(
        &self,
        binding: &ProviderBinding,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        let content = binding
            .profile
            .environment
            .as_ref()
            .ok_or(ProfileStoreError::NotFound)?
            .sandbox
            .clone();
        Ok(SandboxProfileRevision {
            profile_id: SandboxProfileId(format!("sbp_{}", identity(binding, &content)?)),
            revision: 1,
            digest: content_digest(&content)?,
            content,
            published_at: binding.created_at.clone(),
        })
    }
}
fn identity(
    binding: &ProviderBinding,
    content: &impl serde::Serialize,
) -> Result<String, ProfileStoreError> {
    Ok(content_digest(&(
        &binding.binding_id,
        &binding.owner,
        content,
    ))?)
}
