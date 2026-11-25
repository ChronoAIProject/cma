//! Exact transfer of a deleted partition between immutable profile bindings.
use super::{
    BindingConversation, BindingConversationState, BindingError, BindingService, ProviderBinding,
};
use crate::{resource_access::ResourceOwner, AgentId, SubjectId, WorktreeId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileTransferRequest {
    pub source_binding_id: String,
    pub destination_binding_id: String,
    pub conversation_id: String,
    pub expected_agent_id: AgentId,
    pub expected_generation: u64,
    pub expected_destination_generation: u64,
    pub expected_incarnation: u64,
    pub owner: ResourceOwner,
    pub manager: SubjectId,
    pub operation_id: String,
    pub request_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileTransferReceipt {
    pub request: ProfileTransferRequest,
    pub source_conversation_id: String,
    pub destination_generation: u64,
    pub destination_agent_id: AgentId,
    pub destination_worktree_id: WorktreeId,
}

impl ProfileTransferRequest {
    pub fn validate(&self) -> Result<(), BindingError> {
        if self.source_binding_id == self.destination_binding_id
            || !super::model::valid_id(&self.source_binding_id, 1)
            || !super::model::valid_id(&self.destination_binding_id, 1)
            || !super::model::valid_id(&self.conversation_id, 8)
            || self.expected_generation == 0
            || self.expected_destination_generation == 0
            || self.expected_agent_id.as_str().is_empty()
            || self.expected_agent_id.as_str().len() > 128
            || self.manager.as_str().is_empty()
            || [&self.operation_id, &self.request_digest]
                .into_iter()
                .any(|hash| {
                    hash.len() != 64
                        || !hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        {
            return Err(BindingError::InvalidRequest(
                "invalid profile transfer".into(),
            ));
        }
        Ok(())
    }
    pub fn authorize(
        &self,
        source: &ProviderBinding,
        destination: &ProviderBinding,
    ) -> Result<(), BindingError> {
        if source.binding_id != self.source_binding_id
            || destination.binding_id != self.destination_binding_id
            || source.manager != self.manager
            || destination.manager != self.manager
            || source.resource_owner() != self.owner
            || destination.resource_owner() != self.owner
        {
            return Err(BindingError::NotFound);
        }
        Ok(())
    }
}

impl ProfileTransferReceipt {
    pub fn validate_source(&self, source: &BindingConversation) -> Result<(), String> {
        self.request.validate().map_err(|error| error.to_string())?;
        let owner = match &self.request.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if owner != &source.owner
            || self.request.source_binding_id != source.binding_id
            || self.request.expected_generation != source.generation
            || self.request.expected_agent_id != source.agent_id
            || self.source_conversation_id != source.conversation_id
            || source.state != BindingConversationState::Retired
            || self.destination_generation != self.request.expected_destination_generation
            || self.destination_agent_id == source.agent_id
            || self.destination_agent_id.as_str().is_empty()
            || self.destination_worktree_id.as_str().is_empty()
        {
            return Err("profile transfer receipt disagrees with retired source".into());
        }
        Ok(())
    }
}

impl BindingService {
    pub async fn transfer_profile_conversation(
        &self,
        caller: &crate::Caller,
        mut request: ProfileTransferRequest,
        enabled: bool,
    ) -> Result<ProfileTransferReceipt, BindingError> {
        super::service::require(caller)?;
        request.manager = caller.subject.clone();
        request.validate()?;
        self.store
            .transfer_profile_conversation(request, enabled)
            .await
    }
}
