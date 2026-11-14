//! Partition an existing budget without increasing its total concurrency.
use super::*;

pub(super) struct Reservation {
    parent: Arc<BackgroundBudget>,
    permit: Option<OwnedSemaphorePermit>,
}

impl BackgroundBudget {
    /// Reserve before workers start. Busy or invalid reservations fail closed;
    /// they must never fall back to a lane that can still starve recovery.
    pub(crate) fn reserve(self: &Arc<Self>, capacity: usize) -> Option<Arc<Self>> {
        let mut priority = self.priority.lock().unwrap();
        if capacity == 0 || capacity >= self.capacity - priority.reserved {
            return None;
        }
        let count = u32::try_from(capacity).ok()?;
        let permit = self.slots.clone().try_acquire_many_owned(count).ok()?;
        priority.reserved += capacity;
        let mut reserved = Self::with_concurrency(capacity);
        Arc::get_mut(&mut reserved).unwrap()._reservation = Some(Reservation {
            parent: self.clone(),
            permit: Some(permit),
        });
        Some(reserved)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // Return capacity and its foreground accounting under the same lock.
        // Work clones keep this reservation alive through the final attempt.
        let mut priority = self.parent.priority.lock().unwrap();
        let permit = self.permit.take().unwrap();
        priority.reserved -= permit.num_permits();
        drop(permit);
    }
}
