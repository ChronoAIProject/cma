//! Account retained live fragments; decoder scratch and projections are separate.

use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use layer3_adapter::ingress_budget::{
    sizing::component_shared_owner_bytes, IngressBudget, IngressClass, IngressOwnershipPolicy,
    IngressReservation, SharedComponentOwned,
};
use tokio::sync::Semaphore;

use super::live::AgentLiveFragment;

const PROCESS_BYTES: usize = 32 * 1024 * 1024;

pub(super) struct LiveBudget {
    local: Arc<Semaphore>,
    aggregate: Option<IngressBudget>,
    pre_admit: bool,
}

impl LiveBudget {
    pub(super) fn process() -> Arc<Self> {
        static BUDGET: OnceLock<Arc<LiveBudget>> = OnceLock::new();
        BUDGET
            .get_or_init(|| Arc::new(Self::new(PROCESS_BYTES)))
            .clone()
    }

    pub(super) fn new(bytes: usize) -> Self {
        Self {
            local: Arc::new(Semaphore::new(bytes)),
            aggregate: None,
            pre_admit: false,
        }
    }

    pub(super) fn process_coordinated() -> Arc<Self> {
        Arc::new(Self {
            local: Self::process().local.clone(),
            aggregate: Some(IngressBudget::process()),
            pre_admit: false,
        })
    }

    pub(super) fn process_owned(policy: &IngressOwnershipPolicy) -> Arc<Self> {
        Arc::new(Self {
            local: Self::process().local.clone(),
            aggregate: Some(policy.budget(IngressClass::Data)),
            pre_admit: true,
        })
    }

    pub(super) fn copy_chunk(
        &self,
        fragment: &AgentLiveFragment,
        delta: &str,
    ) -> Option<AgentLiveDelivery> {
        if !self.pre_admit {
            return self.retain(copy_fragment(fragment, delta));
        }
        let bytes = copied_bytes(fragment, delta.len())?;
        let permit = IngressReservation::try_acquire(&self.local, self.aggregate.as_ref(), bytes)?;
        Some(AgentLiveDelivery(
            Delivery::Admitted(permit.initialize_shared(|| copy_fragment(fragment, delta))),
            tokio::time::Instant::now(),
        ))
    }

    fn retain(&self, fragment: AgentLiveFragment) -> Option<AgentLiveDelivery> {
        // The temporary producer fragment already exists. Reserve before moving
        // it into retained shared storage, using its actual owned capacities.
        let bytes = retained_bytes(&fragment)?;
        let permit =
            IngressReservation::try_acquire(&self.local, self.aggregate.as_ref(), bytes as usize)?;
        Some(AgentLiveDelivery(
            Delivery::Reserved(Arc::new(Reserved {
                fragment,
                _permit: permit,
            })),
            tokio::time::Instant::now(),
        ))
    }

    #[cfg(test)]
    pub(super) fn available(&self) -> usize {
        self.local.available_permits()
    }
}

fn copy_fragment(fragment: &AgentLiveFragment, delta: &str) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: fragment.agent_id.clone(),
        runtime_generation: fragment.runtime_generation,
        turn_sequence: fragment.turn_sequence,
        after_seq: fragment.after_seq,
        message_id: fragment.message_id.clone(),
        kind: fragment.kind,
        delta: delta.to_owned(),
    }
}

fn copied_bytes(fragment: &AgentLiveFragment, delta_bytes: usize) -> Option<usize> {
    // These copies request exactly their lengths; P1 sizes both owner backings.
    component_shared_owner_bytes::<AgentLiveFragment>()
        .ok()?
        .checked_add(fragment.agent_id.0.len())?
        .checked_add(fragment.message_id.len())?
        .checked_add(delta_bytes)
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
    _permit: IngressReservation,
}

#[derive(Clone)]
enum Delivery {
    Legacy(AgentLiveFragment),
    Reserved(Arc<Reserved>),
    Admitted(SharedComponentOwned<AgentLiveFragment>),
}

/// Immutable source data. Enabled deliveries share both storage and reservation
/// across subscribers and deferred frames, until the final clone is dropped.
#[derive(Clone)]
pub struct AgentLiveDelivery(Delivery, tokio::time::Instant);

impl Deref for AgentLiveDelivery {
    type Target = AgentLiveFragment;

    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Delivery::Legacy(fragment) => fragment,
            Delivery::Reserved(fragment) => &fragment.fragment,
            Delivery::Admitted(fragment) => fragment,
        }
    }
}

impl From<AgentLiveFragment> for AgentLiveDelivery {
    fn from(fragment: AgentLiveFragment) -> Self {
        Self(Delivery::Legacy(fragment), tokio::time::Instant::now())
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

#[cfg(test)]
#[path = "conversation_recorder/ingress_live_tests.rs"]
mod ingress_tests;

#[cfg(test)]
#[path = "conversation_recorder/ingress_live_deallocation_tests.rs"]
mod deallocation_tests;
