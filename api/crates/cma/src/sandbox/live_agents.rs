use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use layer2_core::sandbox::{
    AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeRegistry, AgentRuntimeTarget,
    AgentUpdateRecorder,
};
use layer2_core::{AgentId, RuntimeCapabilities};
use layer3_adapter::{ConversationInputSender, ConversationUpdate};
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

mod authority;
use authority::{accept_authority, fence_authority, validate_authority};
mod budget;
mod capacity;
mod ownership;
mod publication;
mod recovery;
mod recovery_output;
mod updates;
pub(crate) use recovery::{
    DurableAgentConnectionLossHandler, LiveAgentAttachmentIdentity, LiveAgentConnectionLossHandler,
};
#[cfg(test)]
pub(crate) use updates::record_update as record_update_for_test;
#[cfg(test)]
pub(crate) use updates::spawn_update_task;
#[cfg(not(test))]
use updates::spawn_update_task;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LiveAgentRegistryError {
    #[error("a newer runtime generation is already attached")]
    StaleGeneration,
    #[error("the runtime generation is bound to another conversation")]
    ConversationMismatch,
    #[error("the live attachment is no longer registered")]
    NotAttached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveAgentTarget {
    pub agent_id: AgentId,
    pub conversation_id: String,
    pub runtime_generation: u64,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct LiveAgentConnection {
    pub capabilities: RuntimeCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveAgentAttachment {
    agent_id: AgentId,
    conversation_id: String,
    runtime_generation: u64,
    authority_fence: u64,
    nonce: u64,
    inserted: bool,
}

impl LiveAgentAttachment {
    pub(crate) fn inserted(&self) -> bool {
        self.inserted
    }
}

struct RegistryEntry {
    manager_authority: Option<Arc<layer2_core::agent_ownership::Authority>>,
    conversation_id: String,
    runtime_generation: u64,
    authority_fence: u64,
    capabilities: RuntimeCapabilities,
    input: AttachmentInput,
    published: bool,
    nonce: u64,
    task: AbortHandle,
}

enum AttachmentInput {
    Opening,
    Connected {
        input: ConversationInputSender,
        activation: Option<oneshot::Sender<LiveAgentAttachmentIdentity>>,
    },
}

impl AttachmentInput {
    fn sender(&self) -> Option<&ConversationInputSender> {
        match self {
            Self::Opening => None,
            Self::Connected { input, .. } => Some(input),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct AuthorityWatermark {
    runtime_generation: u64,
    authority_fence: u64,
    fenced: bool,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<AgentId, RegistryEntry>,
    staged: HashMap<AgentId, RegistryEntry>,
    watermarks: HashMap<AgentId, AuthorityWatermark>,
}

#[derive(Default)]
pub(crate) struct LiveAgentRegistry {
    ownership: OnceLock<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    state: Mutex<RegistryState>,
    next_nonce: AtomicU64,
    loss_handler: OnceLock<Arc<dyn LiveAgentConnectionLossHandler>>,
}

impl LiveAgentRegistry {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn attach(
        self: &Arc<Self>,
        agent_id: AgentId,
        conversation_id: String,
        runtime_generation: u64,
        authority_fence: u64,
        capabilities: RuntimeCapabilities,
        input: ConversationInputSender,
        updates: mpsc::Receiver<ConversationUpdate>,
        recorder: Arc<dyn AgentUpdateRecorder>,
    ) -> Result<LiveAgentAttachment, LiveAgentRegistryError> {
        let manager_authority = self.attachment_owner(&agent_id)?;
        if runtime_generation == 0 || authority_fence == 0 {
            return Err(LiveAgentRegistryError::StaleGeneration);
        }
        let mut state = self.lock();
        validate_authority(&state, &agent_id, runtime_generation, authority_fence)?;
        if let Some(existing) = state.staged.get(&agent_id) {
            validate_replacement(
                existing,
                &conversation_id,
                runtime_generation,
                authority_fence,
            )?;
            if exact_authority(existing, runtime_generation, authority_fence) {
                return Ok(attachment(&agent_id, existing, false));
            }
        }
        if let Some(existing) = state.entries.get(&agent_id) {
            validate_replacement(
                existing,
                &conversation_id,
                runtime_generation,
                authority_fence,
            )?;
            if exact_authority(existing, runtime_generation, authority_fence) {
                return Ok(attachment(&agent_id, existing, false));
            }
        }
        accept_authority(&mut state, &agent_id, runtime_generation, authority_fence);

        let nonce = self.next_nonce.fetch_add(1, Ordering::Relaxed) + 1;
        let (activate, activated) = oneshot::channel();
        let task = spawn_update_task(Arc::downgrade(self), activated, updates, recorder);
        let entry = RegistryEntry {
            manager_authority,
            conversation_id: conversation_id.clone(),
            runtime_generation,
            authority_fence,
            capabilities,
            input: AttachmentInput::Connected {
                input,
                activation: Some(activate),
            },
            published: false,
            nonce,
            task,
        };
        if let Some(replaced) = state.staged.insert(agent_id.clone(), entry) {
            replaced.task.abort();
        }
        Ok(LiveAgentAttachment {
            agent_id,
            conversation_id,
            runtime_generation,
            authority_fence,
            nonce,
            inserted: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn resolve(&self, target: &LiveAgentTarget) -> Option<LiveAgentConnection> {
        self.lock().entries.get(&target.agent_id).and_then(|entry| {
            (entry.published
                && entry.conversation_id == target.conversation_id
                && entry.runtime_generation == target.runtime_generation)
                .then(|| LiveAgentConnection {
                    capabilities: entry.capabilities.clone(),
                })
        })
    }

    pub(crate) fn published_target_is_attached(&self, target: &LiveAgentTarget) -> bool {
        target_matches(&self.lock().entries, target)
    }

    pub(crate) fn target_is_attached(&self, target: &LiveAgentTarget) -> bool {
        let state = self.lock();
        target_matches(&state.entries, target) || target_matches(&state.staged, target)
    }

    pub(crate) fn attachment(
        &self,
        target: &LiveAgentTarget,
        authority_fence: u64,
    ) -> Result<Option<LiveAgentAttachment>, LiveAgentRegistryError> {
        if authority_fence == 0 {
            return Err(LiveAgentRegistryError::StaleGeneration);
        }
        let mut state = self.lock();
        validate_authority(
            &state,
            &target.agent_id,
            target.runtime_generation,
            authority_fence,
        )?;
        accept_authority(
            &mut state,
            &target.agent_id,
            target.runtime_generation,
            authority_fence,
        );
        if let Some(entry) = state.staged.get_mut(&target.agent_id).filter(|entry| {
            entry.input.sender().is_some()
                && (matches!(
                    entry.input,
                    AttachmentInput::Connected {
                        activation: Some(_),
                        ..
                    }
                ) || entry.authority_fence == authority_fence)
                && entry.conversation_id == target.conversation_id
                && entry.runtime_generation == target.runtime_generation
        }) {
            if entry.authority_fence > authority_fence {
                return Err(LiveAgentRegistryError::StaleGeneration);
            }
            entry.authority_fence = authority_fence;
            let attachment = attachment(&target.agent_id, entry, false);
            accept_authority(
                &mut state,
                &target.agent_id,
                target.runtime_generation,
                authority_fence,
            );
            return Ok(Some(attachment));
        }
        let Some(entry) = state.entries.get(&target.agent_id).filter(|entry| {
            entry.conversation_id == target.conversation_id
                && entry.runtime_generation == target.runtime_generation
        }) else {
            return Ok(None);
        };
        if entry.authority_fence > authority_fence {
            return Err(LiveAgentRegistryError::StaleGeneration);
        }
        let attachment = attachment(&target.agent_id, entry, false);
        accept_authority(
            &mut state,
            &target.agent_id,
            target.runtime_generation,
            authority_fence,
        );
        Ok(Some(attachment))
    }

    pub(crate) fn rollback(&self, attachment: &LiveAgentAttachment) {
        if attachment.inserted {
            let mut state = self.lock();
            let matches = exact_entry(&state.staged, attachment).is_some();
            if matches {
                if let Some(entry) = state.staged.remove(&attachment.agent_id) {
                    entry.task.abort();
                }
            }
        }
    }

    pub(crate) fn fence(
        &self,
        agent_id: &AgentId,
        runtime_generation: Option<u64>,
        authority_fence: u64,
    ) {
        let mut state = self.lock();
        let generation = runtime_generation.unwrap_or_else(|| {
            state
                .watermarks
                .get(agent_id)
                .map_or(0, |watermark| watermark.runtime_generation)
        });
        if !fence_authority(&mut state, agent_id, generation, authority_fence) {
            return;
        }
        remove_fenced_entry(
            &mut state.entries,
            agent_id,
            runtime_generation,
            authority_fence,
        );
        remove_fenced_entry(
            &mut state.staged,
            agent_id,
            runtime_generation,
            authority_fence,
        );
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for LiveAgentRegistry {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let state = self.lock();
        let entry = state
            .entries
            .get(&target.agent_id)
            .ok_or(AgentRuntimeError::NotFound)?;
        if entry.runtime_generation != target.runtime_generation {
            return Err(AgentRuntimeError::StaleGeneration);
        }
        if !entry.published || entry.conversation_id != target.conversation_id {
            return Err(AgentRuntimeError::NotFound);
        }
        if entry
            .manager_authority
            .as_ref()
            .is_some_and(|owner| !owner.is_current())
        {
            return Err(AgentRuntimeError::StaleGeneration);
        }
        Ok(AgentRuntimeConnection {
            capabilities: entry.capabilities.clone(),
            input: entry
                .input
                .sender()
                .ok_or(AgentRuntimeError::NotFound)?
                .clone(),
        })
    }

    async fn request_reconnect(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<(), AgentRuntimeError> {
        self.reconnect_target(target).await
    }

    async fn fence(
        &self,
        agent_id: &AgentId,
        authority_fence: u64,
    ) -> Result<(), AgentRuntimeError> {
        if authority_fence == 0 {
            return Err(AgentRuntimeError::StaleGeneration);
        }
        LiveAgentRegistry::fence(self, agent_id, None, authority_fence);
        Ok(())
    }
}

fn exact_entry<'a>(
    entries: &'a HashMap<AgentId, RegistryEntry>,
    attachment: &LiveAgentAttachment,
) -> Option<&'a RegistryEntry> {
    entries.get(&attachment.agent_id).filter(|entry| {
        entry.conversation_id == attachment.conversation_id
            && entry.runtime_generation == attachment.runtime_generation
            && entry.authority_fence == attachment.authority_fence
            && entry.nonce == attachment.nonce
    })
}

fn validate_replacement(
    existing: &RegistryEntry,
    conversation_id: &str,
    runtime_generation: u64,
    authority_fence: u64,
) -> Result<(), LiveAgentRegistryError> {
    if existing.runtime_generation > runtime_generation
        || (existing.runtime_generation == runtime_generation
            && existing.authority_fence > authority_fence)
    {
        return Err(LiveAgentRegistryError::StaleGeneration);
    }
    if existing.runtime_generation == runtime_generation
        && existing.conversation_id != conversation_id
    {
        return Err(LiveAgentRegistryError::ConversationMismatch);
    }
    Ok(())
}

fn exact_authority(entry: &RegistryEntry, runtime_generation: u64, authority_fence: u64) -> bool {
    entry.runtime_generation == runtime_generation && entry.authority_fence == authority_fence
}

fn attachment(agent_id: &AgentId, entry: &RegistryEntry, inserted: bool) -> LiveAgentAttachment {
    LiveAgentAttachment {
        agent_id: agent_id.clone(),
        conversation_id: entry.conversation_id.clone(),
        runtime_generation: entry.runtime_generation,
        authority_fence: entry.authority_fence,
        nonce: entry.nonce,
        inserted,
    }
}

fn target_matches(entries: &HashMap<AgentId, RegistryEntry>, target: &LiveAgentTarget) -> bool {
    entries.get(&target.agent_id).is_some_and(|entry| {
        entry.conversation_id == target.conversation_id
            && entry.runtime_generation == target.runtime_generation
            && entry
                .manager_authority
                .as_ref()
                .is_none_or(|owner| owner.is_current())
    })
}

fn remove_fenced_entry(
    entries: &mut HashMap<AgentId, RegistryEntry>,
    agent_id: &AgentId,
    runtime_generation: Option<u64>,
    authority_fence: u64,
) {
    let should_remove = entries.get(agent_id).is_some_and(|entry| {
        runtime_generation.is_none_or(|generation| entry.runtime_generation == generation)
            && entry.authority_fence <= authority_fence
    });
    if should_remove {
        if let Some(entry) = entries.remove(agent_id) {
            entry.task.abort();
        }
    }
}
