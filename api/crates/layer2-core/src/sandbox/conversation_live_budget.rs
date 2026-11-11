//! Account retained live fragments; decoder scratch and projections are separate.

use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::live::AgentLiveFragment;

const PROCESS_BYTES: usize = 32 * 1024 * 1024;

pub(super) struct LiveBudget(Arc<Semaphore>);

impl LiveBudget {
    pub(super) fn process() -> Arc<Self> {
        static BUDGET: OnceLock<Arc<LiveBudget>> = OnceLock::new();
        BUDGET
            .get_or_init(|| Arc::new(Self::new(PROCESS_BYTES)))
            .clone()
    }

    pub(super) fn new(bytes: usize) -> Self {
        Self(Arc::new(Semaphore::new(bytes)))
    }

    pub(super) fn retain(&self, fragment: AgentLiveFragment) -> Option<AgentLiveDelivery> {
        // The temporary producer fragment already exists. Reserve before moving
        // it into retained shared storage, using its actual owned capacities.
        let bytes = retained_bytes(&fragment)?;
        let permit = self.0.clone().try_acquire_many_owned(bytes).ok()?;
        Some(AgentLiveDelivery(Delivery::Reserved(Arc::new(Reserved {
            fragment,
            _permit: permit,
        }))))
    }

    #[cfg(test)]
    pub(super) fn available(&self) -> usize {
        self.0.available_permits()
    }
}

pub(super) fn retained_bytes(fragment: &AgentLiveFragment) -> Option<u32> {
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
    _permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
enum Delivery {
    Legacy(AgentLiveFragment),
    Reserved(Arc<Reserved>),
}

/// Immutable source data. Enabled deliveries share both storage and reservation
/// across subscribers and deferred frames, until the final clone is dropped.
#[derive(Clone)]
pub struct AgentLiveDelivery(Delivery);

impl Deref for AgentLiveDelivery {
    type Target = AgentLiveFragment;

    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Delivery::Legacy(fragment) => fragment,
            Delivery::Reserved(fragment) => &fragment.fragment,
        }
    }
}

impl From<AgentLiveFragment> for AgentLiveDelivery {
    fn from(fragment: AgentLiveFragment) -> Self {
        Self(Delivery::Legacy(fragment))
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
    /// Preserve legacy batching. Budgeted source data remains immutable: copying
    /// it into a new coalesced allocation would need a separate byte reservation.
    pub fn coalesce_legacy(&mut self, next: &Self, max_bytes: usize) -> bool {
        let Delivery::Legacy(previous) = &mut self.0 else {
            return false;
        };
        if !matches!(next.0, Delivery::Legacy(_))
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
