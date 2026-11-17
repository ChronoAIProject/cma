use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use layer3_adapter::{ConversationUpdate, SessionUpdate};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::AgentId;

use super::super::{Agent, AgentMessageAssembly};
use super::live_budget::{AgentLiveDelivery, LiveBudget};
#[path = "conversation_head.rs"]
mod head;
pub use head::AgentHeadSubscription;

pub(crate) const LIVE_BUFFER_FRAGMENTS: usize = 256;
const LIVE_FRAGMENT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentLiveFragmentKind {
    Message,
    Thought,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    receiver: broadcast::Receiver<LiveFrame>,
    _sender: Arc<broadcast::Sender<LiveFrame>>,
    admission: Option<tokio::sync::oneshot::Receiver<Result<(), AgentLiveError>>>,
    admission_deadline: std::time::Duration,
}

#[derive(Clone)]
struct LiveFrame {
    fragment: Option<AgentLiveDelivery>,
    authority: Option<Arc<crate::agent_ownership::Authority>>,
}

impl LiveFrame {
    fn current(self) -> Result<AgentLiveDelivery, broadcast::error::RecvError> {
        if self
            .authority
            .is_some_and(|authority| !authority.is_current())
        {
            return Err(broadcast::error::RecvError::Lagged(1));
        }
        self.fragment.ok_or(broadcast::error::RecvError::Lagged(1))
    }
}

/// A configured cluster source may relay the local lane from its current owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentLiveError {
    #[error("observation relay capacity is exhausted")]
    Capacity,
    #[error("observation source is unavailable")]
    Unavailable,
}

pub trait AgentLiveSource: Send + Sync {
    fn subscribe(&self, id: &AgentId) -> Option<AgentLiveSubscription>;
    fn try_subscribe(&self, id: &AgentId) -> Result<Option<AgentLiveSubscription>, AgentLiveError> {
        Ok(self.subscribe(id))
    }
}

#[derive(Clone)]
pub struct AgentLivePublisher(Arc<broadcast::Sender<LiveFrame>>);

impl AgentLivePublisher {
    pub fn publish(
        &self,
        fragment: impl Into<AgentLiveDelivery>,
        authority: Arc<crate::agent_ownership::Authority>,
    ) {
        let _ = self.0.send(LiveFrame {
            fragment: Some(fragment.into()),
            authority: Some(authority),
        });
    }

    pub fn gap(&self) {
        let _ = self.0.send(LiveFrame {
            fragment: None,
            authority: None,
        });
    }

    pub async fn closed(&self) {
        self.0.closed().await;
    }
}

impl AgentLiveSubscription {
    pub fn with_admission_deadline(
        mut self,
        admission: tokio::sync::oneshot::Receiver<Result<(), AgentLiveError>>,
        deadline: std::time::Duration,
    ) -> Self {
        self.admission = Some(admission);
        self.admission_deadline = deadline;
        self
    }
    pub async fn admit(&mut self) -> Result<(), AgentLiveError> {
        if let Some(admission) = self.admission.take() {
            // Only proven capacity refuses a new observer. Owner election or a
            // disconnected live lane must retain the existing durable replay
            // path while the source reconnects in the background.
            if let Ok(Ok(Err(AgentLiveError::Capacity))) =
                tokio::time::timeout(self.admission_deadline, admission).await
            {
                return Err(AgentLiveError::Capacity);
            }
        }
        Ok(())
    }
    pub const BUFFER_FRAGMENTS: usize = LIVE_BUFFER_FRAGMENTS;
    pub fn channel() -> (AgentLivePublisher, Self) {
        let sender = Arc::new(broadcast::channel(LIVE_BUFFER_FRAGMENTS).0);
        let subscription = Self {
            admission: None,
            admission_deadline: std::time::Duration::ZERO,
            receiver: sender.subscribe(),
            _sender: sender.clone(),
        };
        (AgentLivePublisher(sender), subscription)
    }

    pub async fn recv(&mut self) -> Result<AgentLiveDelivery, broadcast::error::RecvError> {
        self.receiver.recv().await?.current()
    }

    pub fn try_recv(&mut self) -> Result<AgentLiveDelivery, broadcast::error::TryRecvError> {
        self.receiver
            .try_recv()?
            .current()
            .map_err(|error| match error {
                broadcast::error::RecvError::Lagged(count) => {
                    broadcast::error::TryRecvError::Lagged(count)
                }
                broadcast::error::RecvError::Closed => broadcast::error::TryRecvError::Closed,
            })
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
    heads: head::AgentHeadHub,
    budget: Option<Arc<LiveBudget>>,
    lanes: Mutex<BTreeMap<AgentId, Weak<broadcast::Sender<LiveFrame>>>>,
}

impl AgentLiveHub {
    pub(super) fn subscribe_head(&self, id: &AgentId) -> AgentHeadSubscription {
        self.heads.subscribe(id)
    }
    pub(super) fn publish_head(&self, id: &AgentId) {
        self.heads.publish(id);
    }
    pub(super) fn with_budget(budget: Arc<LiveBudget>) -> Self {
        Self {
            budget: Some(budget),
            heads: Default::default(),
            lanes: Mutex::default(),
        }
    }

    #[cfg(feature = "test-support")]
    pub(super) fn subscriber_count(&self, id: &AgentId) -> usize {
        self.lanes
            .lock()
            .unwrap()
            .get(id)
            .and_then(Weak::upgrade)
            .map_or(0, |lane| lane.receiver_count())
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
            admission: None,
            admission_deadline: std::time::Duration::ZERO,
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
            let _ = sender.send(LiveFrame {
                fragment: delivery,
                authority: crate::agent_ownership::execution_authority(),
            });
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

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn local_subscription_without_admission_receiver_never_waits() {
        let recorder = super::super::DurableAgentUpdateRecorder::new(Arc::new(
            crate::sandbox::store::InMemorySandboxStore::default(),
        ));
        let mut subscription = recorder.subscribe_local(&AgentId("local-admission".into()));
        let started = Instant::now();
        subscription.admit().await.unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_admission_falls_back_at_the_supplied_deadline_once() {
        let (_, subscription) = AgentLiveSubscription::channel();
        let (_sender, receiver) = tokio::sync::oneshot::channel();
        let deadline = Duration::from_secs(3);
        let mut subscription = subscription.with_admission_deadline(receiver, deadline);
        let started = Instant::now();
        subscription.admit().await.unwrap();
        assert_eq!(started.elapsed(), deadline);
        subscription.admit().await.unwrap();
        assert_eq!(started.elapsed(), deadline, "admission is not repeated");
    }

    #[tokio::test(start_paused = true)]
    async fn capacity_refusal_is_typed_and_disconnected_sources_allow_replay() {
        for outcome in [
            Ok(()),
            Err(AgentLiveError::Capacity),
            Err(AgentLiveError::Unavailable),
        ] {
            let (_, subscription) = AgentLiveSubscription::channel();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let mut subscription =
                subscription.with_admission_deadline(receiver, Duration::from_secs(3));
            sender.send(outcome).unwrap();
            let expected = if outcome == Err(AgentLiveError::Capacity) {
                outcome
            } else {
                Ok(())
            };
            assert_eq!(subscription.admit().await, expected);
        }
    }
}
