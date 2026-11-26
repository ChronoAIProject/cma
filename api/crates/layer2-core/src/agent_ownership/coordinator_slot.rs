use super::*;
use crate::agent_ownership::lease_operation::LeaseWrite;

#[derive(Default)]
pub(super) struct Slot {
    pub authority: Option<Arc<Authority>>,
    pub acquisition: Option<Arc<LeaseWrite>>,
}

impl Slot {
    pub(super) fn owns(&self, authority: &Arc<Authority>) -> bool {
        self.authority
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, authority))
    }
}
