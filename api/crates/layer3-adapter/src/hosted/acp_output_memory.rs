//! Own raw payload and queue-node reservations through their actual lifetime.

use std::collections::{LinkedList, VecDeque};
use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::HeldOutput;
use crate::AdapterError;

const PROCESS_BYTES: usize = 16 * 1024 * 1024;
// A linked node has two pointers plus its value. Include conservative layout
// allowance; this is requested storage accounting, not allocator/RSS telemetry.
const NODE_BYTES: usize = size_of::<HeldOutput>() + 2 * size_of::<usize>() + 128;

pub(super) fn process_budget(enabled: bool) -> Option<Arc<Semaphore>> {
    static BUDGET: OnceLock<Arc<Semaphore>> = OnceLock::new();
    enabled.then(|| {
        BUDGET
            .get_or_init(|| Arc::new(Semaphore::new(PROCESS_BYTES)))
            .clone()
    })
}

pub(super) fn reserve(
    budget: Option<&Arc<Semaphore>>,
    payload_bytes: usize,
) -> crate::Result<Option<OwnedSemaphorePermit>> {
    let Some(budget) = budget else {
        return Ok(None);
    };
    let weight = payload_bytes
        .checked_add(NODE_BYTES)
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or_else(capacity)?;
    budget
        .clone()
        .try_acquire_many_owned(weight)
        .map(Some)
        .map_err(|_| capacity())
}

fn capacity() -> AdapterError {
    AdapterError::AtCapacity("retained raw output storage capacity exhausted".into())
}

// Independent node ownership avoids a drained VecDeque retaining uncharged
// capacity. The legacy queue keeps its original representation when disabled.
#[allow(clippy::linkedlist)]
pub(super) enum OutputQueue {
    Legacy(VecDeque<HeldOutput>),
    Bounded(LinkedList<HeldOutput>),
}

impl OutputQueue {
    pub(super) fn new(bounded: bool) -> Self {
        if bounded {
            Self::Bounded(LinkedList::new())
        } else {
            Self::Legacy(VecDeque::new())
        }
    }

    pub(super) fn push_back(&mut self, held: HeldOutput) {
        match self {
            Self::Legacy(queue) => queue.push_back(held),
            Self::Bounded(queue) => queue.push_back(held),
        }
    }

    pub(super) fn pop_front(&mut self) -> Option<HeldOutput> {
        match self {
            Self::Legacy(queue) => queue.pop_front(),
            Self::Bounded(queue) => queue.pop_front(),
        }
    }
}

impl Drop for OutputQueue {
    fn drop(&mut self) {
        // pop_front frees each linked allocation before returning its value;
        // only then may dropping that value release its payload reservation.
        while let Some(held) = self.pop_front() {
            drop(held);
        }
    }
}

#[cfg(test)]
#[path = "acp_output_memory_tests.rs"]
mod tests;
