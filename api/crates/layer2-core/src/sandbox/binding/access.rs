use super::{
    BindingConversation, BindingConversationState, BindingError, BindingState, BindingStore,
    BindingWork, ProviderBinding,
};
use crate::resource_access::ResourceOwner;
use crate::{AgentId, Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;

/// Created only from retained binding authority, never from request JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingPrincipal {
    binding_id: String,
    owner: ResourceOwner,
    generation: u64,
    key_sha256: String,
}

impl BindingPrincipal {
    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }
    pub fn owner(&self) -> &SubjectId {
        match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        }
    }
    pub fn resource_owner(&self) -> &ResourceOwner {
        &self.owner
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn key_sha256(&self) -> &str {
        &self.key_sha256
    }
    pub fn caller(&self) -> Caller {
        Caller::new(
            self.owner().clone(),
            Authorization::new([Capability::UseBindingConversations]),
        )
    }
}

/// Exact parent/conversation evidence to fence admission and buffered reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingConversationAuthority {
    pub binding: ProviderBinding,
    pub conversation: BindingConversation,
}

#[derive(Clone)]
pub struct BindingAccessService {
    store: Arc<dyn BindingStore>,
}

impl BindingAccessService {
    pub async fn contains_key_hash(
        &self,
        hash: &str,
    ) -> Result<bool, crate::sandbox::store::SandboxStoreError> {
        self.store.contains_key_hash(hash).await
    }
    pub fn new(store: Arc<dyn BindingStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    pub async fn authenticate(&self, hash: &str) -> Result<Option<BindingPrincipal>, BindingError> {
        let Some(binding) = self.store.find_by_key_hash(hash).await? else {
            return Ok(None);
        };
        binding.validate().map_err(BindingError::Corrupt)?;
        if binding.key_sha256 != hash || binding.state == BindingState::Retired {
            return Err(BindingError::Corrupt(
                "binding key index disagrees with authority".into(),
            ));
        }
        Ok(Some(principal(&binding)))
    }

    /// Deferred commands retain the binding generation, so replacement of an
    /// externally owned key never abandons already admitted work.
    pub async fn retained_principal(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<BindingPrincipal, BindingError> {
        let binding = self
            .store
            .binding(id)
            .await?
            .ok_or(BindingError::NotFound)?;
        if !live(&binding) || binding.generation != generation {
            return Err(BindingError::NotFound);
        }
        Ok(principal(&binding))
    }

    pub async fn resolve(
        &self,
        principal: &BindingPrincipal,
        id: &str,
    ) -> Result<BindingConversationAuthority, BindingError> {
        let binding = self.current(principal).await?;
        let conversation = if id.starts_with("agt_") {
            self.store
                .binding_conversation_for_agent(
                    &binding.binding_id,
                    binding.generation,
                    &AgentId(id.to_owned()),
                )
                .await?
        } else {
            // Management creates one canonical conversation for a borrowed
            // Agent, regardless of the transport alias. Context-free turns
            // (including the Gateway test) must resolve the same mapping.
            let id = if binding.existing_agent.is_some() {
                if !super::model::valid_id(id, 8) {
                    return Err(BindingError::NotFound);
                }
                super::EXISTING_AGENT_CONVERSATION
            } else {
                id
            };
            self.store
                .binding_conversation(&binding.binding_id, binding.generation, id)
                .await?
        }
        .ok_or(BindingError::NotFound)?;
        conversation.validate().map_err(BindingError::Corrupt)?;
        if conversation.binding_id != binding.binding_id
            || conversation.generation != binding.generation
            || conversation.owner != binding.owner
            || conversation.sandbox_id != binding.sandbox_id
            || conversation.workspace_id != binding.workspace_id
        {
            return Err(BindingError::Corrupt(
                "binding conversation index disagrees with authority".into(),
            ));
        }
        match conversation.state {
            BindingConversationState::Ready => Ok(BindingConversationAuthority {
                binding,
                conversation,
            }),
            BindingConversationState::Provisioning => Err(BindingError::Conflict),
            _ => Err(BindingError::NotFound),
        }
    }

    pub async fn validate(&self, proof: &BindingConversationAuthority) -> Result<(), BindingError> {
        let current = self
            .resolve(
                &principal(&proof.binding),
                proof.conversation.agent_id.as_str(),
            )
            .await?;
        if current != *proof {
            return Err(BindingError::Conflict);
        }
        Ok(())
    }

    async fn current(&self, principal: &BindingPrincipal) -> Result<ProviderBinding, BindingError> {
        let binding = self
            .store
            .binding(&principal.binding_id)
            .await?
            .ok_or(BindingError::NotFound)?;
        if !live(&binding) || self::principal(&binding) != *principal {
            return Err(BindingError::NotFound);
        }
        Ok(binding)
    }
}

fn principal(binding: &ProviderBinding) -> BindingPrincipal {
    BindingPrincipal {
        binding_id: binding.binding_id.clone(),
        owner: binding.resource_owner(),
        generation: binding.generation,
        key_sha256: binding.key_sha256.clone(),
    }
}
fn live(binding: &ProviderBinding) -> bool {
    binding.state == BindingState::Ready
        || (binding.state == BindingState::Provisioning
            && binding.work == BindingWork::DeliverCredential)
}
