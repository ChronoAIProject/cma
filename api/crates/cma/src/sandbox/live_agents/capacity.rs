//! Exact process-local recovery attachment observation. No authority promotion.
use super::{LiveAgentRegistry, LiveAgentTarget};

impl LiveAgentRegistry {
    /// A current published connection is an idempotency observation only. The
    /// recovery service must still prove its original receipt and current access.
    pub(crate) fn recovery_attachment_is_active(
        &self,
        target: &LiveAgentTarget,
        authority_fence: u64,
    ) -> bool {
        if target.runtime_generation == 0 || authority_fence == 0 {
            return false;
        }
        let state = self.lock();
        let current = state
            .watermarks
            .get(&target.agent_id)
            .is_some_and(|watermark| {
                !watermark.fenced
                    && watermark.runtime_generation == target.runtime_generation
                    && watermark.authority_fence == authority_fence
            });
        current
            && state.entries.get(&target.agent_id).is_some_and(|entry| {
                entry.published
                    && !entry.task.is_finished()
                    && entry.conversation_id == target.conversation_id
                    && entry.runtime_generation == target.runtime_generation
                    && entry.authority_fence == authority_fence
            })
    }
}

#[cfg(test)]
#[path = "capacity_tests.rs"]
mod tests;
