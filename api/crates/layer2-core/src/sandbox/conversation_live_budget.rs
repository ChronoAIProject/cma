//! Account retained live fragments; decoder scratch and projections are separate.

use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use layer3_adapter::ingress_budget::{
    sizing::component_shared_owner_bytes, IngressBudget, IngressReservation, SharedComponentOwned,
};
use tokio::sync::Semaphore;

use super::live::AgentLiveFragment;
use crate::{
    agent_ownership::{Authority, OwnershipToken},
    AgentId,
};

const PROCESS_BYTES: usize = 32 * 1024 * 1024;

pub(super) struct LiveBudget(Arc<Semaphore>, Option<IngressBudget>);

impl LiveBudget {
    pub(super) fn process() -> Arc<Self> {
        static BUDGET: OnceLock<Arc<LiveBudget>> = OnceLock::new();
        BUDGET
            .get_or_init(|| Arc::new(Self::new(PROCESS_BYTES)))
            .clone()
    }

    pub(super) fn new(bytes: usize) -> Self {
        Self(Arc::new(Semaphore::new(bytes)), None)
    }

    pub(super) fn process_coordinated() -> Arc<Self> {
        Arc::new(Self(
            Self::process().0.clone(),
            Some(IngressBudget::process()),
        ))
    }

    pub(super) fn copy_chunk(
        &self,
        source: &AgentLiveFragment,
        chunk: &str,
    ) -> Option<AgentLiveDelivery> {
        let Some(aggregate) = &self.1 else {
            return self.retain_local(copy_fragment(source, chunk));
        };
        let bytes = coordinated_bytes(source, chunk)?;
        let permit = IngressReservation::try_acquire(&self.0, Some(aggregate), bytes)?;
        let fragment = permit.initialize_shared(|| copy_fragment(source, chunk));
        Some(AgentLiveDelivery(
            Delivery::Coordinated(fragment),
            tokio::time::Instant::now(),
            None,
        ))
    }

    fn retain_local(&self, fragment: AgentLiveFragment) -> Option<AgentLiveDelivery> {
        // The temporary producer fragment already exists. Reserve before moving
        // it into retained shared storage, using its actual owned capacities.
        let bytes = retained_bytes(&fragment)?;
        let permit = IngressReservation::try_acquire(&self.0, None, bytes as usize)?;
        Some(AgentLiveDelivery(
            Delivery::Reserved(Arc::new(Reserved {
                fragment,
                _permit: permit,
            })),
            tokio::time::Instant::now(),
            None,
        ))
    }

    #[cfg(test)]
    pub(super) fn available(&self) -> usize {
        self.0.available_permits()
    }
}

fn copy_fragment(source: &AgentLiveFragment, chunk: &str) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: AgentId(Box::<str>::from(source.agent_id.0.as_str()).into_string()),
        runtime_generation: source.runtime_generation,
        turn_sequence: source.turn_sequence,
        after_seq: source.after_seq,
        message_id: Box::<str>::from(source.message_id.as_str()).into_string(),
        kind: source.kind,
        delta: Box::<str>::from(chunk).into_string(),
    }
}

fn coordinated_bytes(source: &AgentLiveFragment, chunk: &str) -> Option<usize> {
    let bytes = component_shared_owner_bytes::<AgentLiveFragment>()
        .ok()?
        .checked_add(source.agent_id.0.len())?
        .checked_add(source.message_id.len())?
        .checked_add(chunk.len())?;
    Some(u32::try_from(bytes).ok()? as usize)
}

fn retained_bytes(fragment: &AgentLiveFragment) -> Option<u32> {
    // Include the owned value, permit, and the two Arc reference counts. The
    // allocator's bookkeeping/rounding and channel slots are not heap bytes here.
    let bytes = std::mem::size_of::<Reserved>()
        .checked_add(2 * std::mem::size_of::<usize>())?
        .checked_add(fragment.agent_id.0.capacity())?
        .checked_add(fragment.message_id.capacity())?
        .checked_add(fragment.delta.capacity())?;
    bytes.try_into().ok()
}

struct Reserved {
    fragment: AgentLiveFragment,
    _permit: IngressReservation,
}

#[derive(Clone)]
enum Delivery {
    Legacy(AgentLiveFragment),
    Reserved(Arc<Reserved>),
    Coordinated(SharedComponentOwned<AgentLiveFragment>),
}

/// Immutable source data. Enabled deliveries share both storage and reservation
/// across subscribers and deferred frames, until the final clone is dropped.
#[derive(Clone)]
pub struct AgentLiveDelivery(Delivery, tokio::time::Instant, Option<Arc<Authority>>);

impl Deref for AgentLiveDelivery {
    type Target = AgentLiveFragment;

    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Delivery::Legacy(fragment) => fragment,
            Delivery::Reserved(fragment) => &fragment.fragment,
            Delivery::Coordinated(fragment) => fragment,
        }
    }
}

impl From<AgentLiveFragment> for AgentLiveDelivery {
    fn from(fragment: AgentLiveFragment) -> Self {
        Self(
            Delivery::Legacy(fragment),
            tokio::time::Instant::now(),
            None,
        )
    }
}

impl PartialEq for AgentLiveDelivery {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}
impl Eq for AgentLiveDelivery {}

impl std::fmt::Debug for AgentLiveDelivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

impl AgentLiveDelivery {
    pub(super) fn with_authority(mut self, authority: Option<Arc<Authority>>) -> Self {
        self.2 = authority;
        self
    }

    pub fn is_current(&self) -> bool {
        self.2
            .as_ref()
            .is_none_or(|authority| authority.is_current())
    }

    pub fn is_current_for(&self, owner: &OwnershipToken) -> bool {
        self.2
            .as_ref()
            .is_none_or(|authority| &authority.token == owner && authority.is_current())
    }

    /// Local monotonic publication time; never part of the wire contract.
    pub fn published_at(&self) -> tokio::time::Instant {
        self.1
    }
    /// Preserve legacy batching. Budgeted source data remains immutable: copying
    /// it into a new coalesced allocation would need a separate byte reservation.
    pub fn coalesce_legacy(&mut self, next: &Self, max_bytes: usize) -> bool {
        let Delivery::Legacy(previous) = &mut self.0 else {
            return false;
        };
        let same_authority = match (&self.2, &next.2) {
            (Some(previous), Some(next)) => Arc::ptr_eq(previous, next),
            (None, None) => true,
            _ => false,
        };
        if !same_authority
            || !matches!(next.0, Delivery::Legacy(_))
            || previous.agent_id != next.agent_id
            || previous.message_id != next.message_id
            || previous.kind != next.kind
            || previous.turn_sequence != next.turn_sequence
            || previous.runtime_generation != next.runtime_generation
            || previous.after_seq != next.after_seq
            || previous.delta.len().saturating_add(next.delta.len()) > max_bytes
        {
            return false;
        }
        previous.delta.push_str(&next.delta);
        true
    }
}

#[cfg(test)]
#[path = "conversation_live_budget_tests.rs"]
mod tests;
