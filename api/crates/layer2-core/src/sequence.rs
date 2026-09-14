//! Process-local sequence reservation for durable resource logs.

use std::collections::HashMap;
use std::sync::Mutex;

/// Monotonic sequence cursors keyed by opaque resource identity.
#[derive(Debug, Default)]
pub struct Sequences {
    issued: Mutex<HashMap<String, u64>>,
}

impl Sequences {
    /// Issue the next sequence, never below the supplied durable floor.
    pub fn next(&self, resource_id: &str, durable_floor: u64) -> u64 {
        let mut issued = self.issued.lock().unwrap_or_else(|lock| lock.into_inner());
        let cursor = issued
            .entry(resource_id.to_owned())
            .or_insert(durable_floor);
        *cursor = (*cursor).max(durable_floor).saturating_add(1);
        *cursor
    }

    /// Observe a newly durable tail without moving a newer local reservation backwards.
    pub fn observe(&self, resource_id: &str, durable_tail: u64) -> u64 {
        let mut issued = self.issued.lock().unwrap_or_else(|lock| lock.into_inner());
        let cursor = issued.entry(resource_id.to_owned()).or_default();
        *cursor = (*cursor).max(durable_tail);
        *cursor
    }

    /// Drop process-local state after the durable owner is gone.
    pub fn forget(&self, resource_id: &str) {
        self.issued
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(resource_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_are_monotonic_and_resource_scoped() {
        let sequences = Sequences::default();
        assert_eq!(sequences.next("a", 4), 5);
        assert_eq!(sequences.next("a", 2), 6);
        assert_eq!(sequences.observe("a", 10), 10);
        assert_eq!(sequences.next("a", 3), 11);
        assert_eq!(sequences.next("b", 0), 1);
        sequences.forget("a");
        assert_eq!(sequences.next("a", 1), 2);
    }
}
