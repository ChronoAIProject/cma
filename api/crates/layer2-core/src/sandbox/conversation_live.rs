use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use layer3_adapter::{ConversationUpdate, SessionUpdate};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::AgentId;

use super::super::{Agent, AgentMessageAssembly};
use super::live_budget::{AgentLiveDelivery, LiveBudget};

pub(crate) const LIVE_BUFFER_FRAGMENTS: usize = 256;
const LIVE_FRAGMENT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLiveFragmentKind {
    Message,
    Thought,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AgentLiveFragment {
    pub agent_id: AgentId,
    pub runtime_generation: u64,
    pub turn_sequence: u64,
    pub after_seq: u64,
    pub message_id: String,
    pub kind: AgentLiveFragmentKind,
    pub delta: String,
}

impl std::fmt::Debug for AgentLiveFragment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentLiveFragment")
            .field("agent_id", &self.agent_id)
            .field("runtime_generation", &self.runtime_generation)
            .field("turn_sequence", &self.turn_sequence)
            .field("after_seq", &self.after_seq)
            .field("message_id", &self.message_id)
            .field("kind", &self.kind)
            .field("delta_bytes", &self.delta.len())
            .finish()
    }
}

/// A live-only lane; lag requires replaying the canonical durable transcript.
pub struct AgentLiveSubscription {
    receiver: broadcast::Receiver<Option<AgentLiveDelivery>>,
    _sender: Arc<broadcast::Sender<Option<AgentLiveDelivery>>>,
}

impl AgentLiveSubscription {
    /// Number of source fragments retained before a subscriber reports lag.
    pub const BUFFER_FRAGMENTS: usize = LIVE_BUFFER_FRAGMENTS;

    pub async fn recv(&mut self) -> Result<AgentLiveDelivery, broadcast::error::RecvError> {
        self.receiver
            .recv()
            .await?
            .ok_or(broadcast::error::RecvError::Lagged(1))
    }

    pub fn try_recv(&mut self) -> Result<AgentLiveDelivery, broadcast::error::TryRecvError> {
        self.receiver
            .try_recv()?
            .ok_or(broadcast::error::TryRecvError::Lagged(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentLiveMessageIdentity {
    generation: u64,
    turn: u64,
    source: Option<String>,
    id: String,
}

#[derive(Default)]
pub(super) struct AgentLiveHub {
    budget: Option<Arc<LiveBudget>>,
    lanes: Mutex<BTreeMap<AgentId, Weak<broadcast::Sender<Option<AgentLiveDelivery>>>>>,
}

impl AgentLiveHub {
    pub(super) fn with_budget(budget: Arc<LiveBudget>) -> Self {
        Self {
            budget: Some(budget),
            lanes: Mutex::default(),
        }
    }

    pub(super) fn subscribe(&self, id: &AgentId) -> AgentLiveSubscription {
        let mut lanes = self.lanes.lock().unwrap_or_else(|error| error.into_inner());
        lanes.retain(|_, sender| sender.strong_count() > 0);
        let sender = lanes.get(id).and_then(Weak::upgrade).unwrap_or_else(|| {
            let sender = Arc::new(broadcast::channel(LIVE_BUFFER_FRAGMENTS).0);
            lanes.insert(id.clone(), Arc::downgrade(&sender));
            sender
        });
        AgentLiveSubscription {
            receiver: sender.subscribe(),
            _sender: sender,
        }
    }

    pub(super) fn publish(&self, fragment: Option<AgentLiveFragment>) {
        let Some(mut fragment) = fragment else { return };
        let sender = self
            .lanes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&fragment.agent_id)
            .and_then(Weak::upgrade);
        let Some(sender) = sender else { return };
        let delta = std::mem::take(&mut fragment.delta);
        let mut remaining = delta.as_str();
        while !remaining.is_empty() {
            let mut end = remaining.len().min(LIVE_FRAGMENT_BYTES);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            fragment.delta = remaining[..end].into();
            let delivery = match &self.budget {
                Some(budget) => budget.retain(fragment.clone()),
                None => Some(fragment.clone().into()),
            };
            // None is an ordered gap, not an acknowledged runtime output. Live
            // consumers already replay durable history after broadcast lag.
            let _ = sender.send(delivery);
            remaining = &remaining[end..];
        }
    }
}

pub(super) fn accepted_fragment(
    current: &Agent,
    previous: &AgentMessageAssembly,
    next: &mut AgentMessageAssembly,
    update: &ConversationUpdate,
    after_seq: u64,
) -> Option<AgentLiveFragment> {
    let generation = current.conversation.runtime_generation?;
    let turn = current.conversation.active_turn?;
    let mut update = update;
    while let ConversationUpdate::WithoutPromptEvidence(inner) = update {
        update = inner;
    }
    let (kind, chunk) = match update {
        ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(chunk)) => {
            (AgentLiveFragmentKind::Message, chunk)
        }
        ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(chunk)) => {
            (AgentLiveFragmentKind::Thought, chunk)
        }
        _ => return None,
    };
    let delta = match kind {
        AgentLiveFragmentKind::Message if previous.message_id == next.message_id => {
            next.content.strip_prefix(&previous.content)?.to_owned()
        }
        AgentLiveFragmentKind::Message => next.content.clone(),
        AgentLiveFragmentKind::Thought => {
            super::super::conversation_fold_support::plain_text(&chunk.content)?
        }
    };
    if delta.is_empty() {
        return None;
    }
    let identity = match kind {
        AgentLiveFragmentKind::Message => &mut next.live_message,
        AgentLiveFragmentKind::Thought => &mut next.live_thought,
    };
    let source = chunk.message_id.as_ref().map(|id| id.0.to_string());
    if identity.as_ref().is_none_or(|identity| {
        identity.generation != generation || identity.turn != turn || identity.source != source
    }) {
        let source_key = source
            .as_ref()
            .map(|source| format!("provided:{source}"))
            .unwrap_or_else(|| format!("sequence:{}", current.conversation.last_seq));
        let key = format!("{}:{generation}:{turn}:{kind:?}:{source_key}", current.id);
        *identity = Some(AgentLiveMessageIdentity {
            generation,
            turn,
            source,
            id: format!("{:x}", Sha256::digest(key.as_bytes())),
        });
    }
    Some(AgentLiveFragment {
        agent_id: current.id.clone(),
        runtime_generation: generation,
        turn_sequence: turn,
        after_seq,
        message_id: identity.as_ref()?.id.clone(),
        kind,
        delta,
    })
}
