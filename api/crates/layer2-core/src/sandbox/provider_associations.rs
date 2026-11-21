//! Retained provider membership, independent of legacy channel-usage history.
//! This read port cannot grant provider authority or mutate resource lifecycle.
use super::binding::{store::conversation_storage_key, BindingConversation, ProviderBinding};
use super::store::{AgentProvenanceSnapshot, PageRequest, SandboxStoreError};
use super::AgentCreationProvenance;
use crate::{AgentId, SubjectId};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

mod service;
pub use service::AgentProviderAssociationService;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAssociationRecord {
    pub conversation: BindingConversation,
    pub binding: ProviderBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProviderAssociationSnapshot {
    pub provenance: AgentProvenanceSnapshot,
    /// At most the requested limit plus one, ordered by conversation storage key.
    pub records: Vec<ProviderAssociationRecord>,
}

#[async_trait::async_trait]
pub trait AgentProviderAssociationStore: Send + Sync {
    /// One read snapshot over Manager-owned ancestry, conversations and parents.
    /// Filter by exact owner + Agent BEFORE limiting; never scan all bindings.
    async fn agent_provider_associations(
        &self,
        owner: &SubjectId,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationSnapshot, SandboxStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentProviderAssociationReading {
    pub agent_id: String,
    pub owner: String,
    /// Identifies the authorized Agent incarnation; not provider activity time.
    pub agent_created_at: String,
    pub items: Vec<ProviderAssociation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderAssociation {
    pub binding_id: String,
    pub binding_generation: u64,
    pub provider_conversation_id: String,
    pub relationship: ProviderAssociationRelationship,
    pub sandbox_id: String,
    pub workspace_id: String,
    pub worktree_id: String,
    pub conversation_state: super::binding::BindingConversationState,
    /// Current parent state, including for a historical conversation generation.
    pub binding_state: ProviderAssociationBindingState,
    pub current_binding_generation: u64,
    pub is_current_generation: bool,
    /// Reservation lifecycle timestamps, NOT first/last accepted event use.
    pub association_created_at: String,
    pub association_updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAssociationRelationship {
    Managed,
    Borrowed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAssociationBindingState {
    Provisioning,
    Ready,
    Failed,
    Retired,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    scope: String,
    after: String,
}
fn scope(creation: &AgentCreationProvenance) -> Result<String, SandboxStoreError> {
    crate::profiles::content_digest(creation).map_err(|_| corrupt())
}
/// Opaque continuation scoped to the exact authorized owner and Agent incarnation.
/// The cursor is not an authorization credential; every page is authorized again.
pub fn association_cursor(
    page: &PageRequest,
    creation: &AgentCreationProvenance,
) -> Result<Option<String>, SandboxStoreError> {
    PageRequest::new(page.limit, None)?;
    let Some(encoded) = &page.cursor else {
        return Ok(None);
    };
    if encoded.is_empty() || encoded.len() > 4096 {
        return Err(SandboxStoreError::InvalidPage);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| SandboxStoreError::InvalidPage)?;
    let cursor: Cursor =
        serde_json::from_slice(&bytes).map_err(|_| SandboxStoreError::InvalidPage)?;
    if URL_SAFE_NO_PAD.encode(bytes) != *encoded
        || cursor.version != 1
        || cursor.scope != scope(creation)?
        || cursor.after.is_empty()
        || cursor.after.len() > 1024
        || cursor.after.chars().any(char::is_control)
    {
        return Err(SandboxStoreError::InvalidPage);
    }
    Ok(Some(cursor.after))
}

pub fn association_page(
    snapshot: &AgentProviderAssociationSnapshot,
    owner: &SubjectId,
    id: &AgentId,
    request: &PageRequest,
) -> Result<AgentProviderAssociationReading, SandboxStoreError> {
    let creation = snapshot.provenance.creation_provenance(id)?;
    let agent = snapshot
        .provenance
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or_else(corrupt)?;
    if &agent.owner != owner {
        return Err(SandboxStoreError::Conflict);
    }
    let after = association_cursor(request, &creation)?;
    if snapshot.records.len() > usize::from(request.limit) + 1 {
        return Err(corrupt());
    }
    let mut previous = after;
    let mut keys = Vec::with_capacity(snapshot.records.len());
    let mut items = Vec::with_capacity(snapshot.records.len());
    for record in &snapshot.records {
        let conversation = &record.conversation;
        let binding = &record.binding;
        conversation
            .validate()
            .map_err(SandboxStoreError::Corrupt)?;
        binding.validate().map_err(SandboxStoreError::Corrupt)?;
        let key = conversation_storage_key(conversation);
        if previous.as_deref().is_some_and(|last| key.as_str() <= last)
            || conversation.owner != *owner
            || binding.owner != *owner
            || binding.resource_owner() != creation.owner
            || conversation.agent_id != *id
            || conversation.sandbox_id != agent.sandbox_id
            || conversation.workspace_id != agent.workspace_id
            || (conversation.worktree_id != agent.worktree_id
                && !agent.work.worktree_recoveries.iter().any(|recovery| {
                    // creation_provenance above already validates the complete
                    // retained owner/Agent/hierarchy/fence recovery chain.
                    recovery.original_worktree.id == conversation.worktree_id
                }))
            || conversation.binding_id != binding.binding_id
            || conversation.generation > binding.generation
            || conversation.borrowed != binding.existing_agent.is_some()
            || binding.existing_agent.as_ref().is_some_and(|existing| {
                existing.agent_id != *id
                    || existing.worktree_id != conversation.worktree_id
                    || conversation.conversation_id != super::binding::EXISTING_AGENT_CONVERSATION
            })
        {
            return Err(corrupt());
        }
        let current = conversation.generation == binding.generation;
        // Old conversations retain their own hierarchy, even after the parent's
        // previous-generation slot has moved on. Never erase retained history.
        if current
            && (conversation.sandbox_id != binding.sandbox_id
                || conversation.workspace_id != binding.workspace_id)
        {
            return Err(corrupt());
        }
        use super::binding::BindingState as B;
        use ProviderAssociationBindingState as S;
        items.push(ProviderAssociation {
            binding_id: binding.binding_id.clone(),
            binding_generation: conversation.generation,
            provider_conversation_id: conversation.conversation_id.clone(),
            relationship: if conversation.borrowed {
                ProviderAssociationRelationship::Borrowed
            } else {
                ProviderAssociationRelationship::Managed
            },
            sandbox_id: conversation.sandbox_id.to_string(),
            workspace_id: conversation.workspace_id.to_string(),
            worktree_id: conversation.worktree_id.to_string(),
            conversation_state: conversation.state,
            binding_state: match binding.state {
                B::Provisioning => S::Provisioning,
                B::Ready => S::Ready,
                B::Failed { .. } => S::Failed,
                B::Retired => S::Retired,
            },
            current_binding_generation: binding.generation,
            is_current_generation: current,
            association_created_at: conversation.created_at.clone(),
            association_updated_at: conversation.updated_at.clone(),
        });
        previous = Some(key.clone());
        keys.push(key);
    }
    let limit = usize::from(request.limit);
    let next = if items.len() > limit {
        let cursor = Cursor {
            version: 1,
            scope: scope(&creation)?,
            after: keys[limit - 1].clone(),
        };
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).map_err(|_| corrupt())?))
    } else {
        None
    };
    items.truncate(limit);
    Ok(AgentProviderAssociationReading {
        agent_id: id.to_string(),
        owner: owner.to_string(),
        agent_created_at: creation.agent_created_at,
        items,
        next,
    })
}
fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("provider association disagrees with retained Agent identity".into())
}
