use super::*;

impl DurableAgentUpdateRecorder {
    #[cfg(feature = "test-support")]
    pub fn publish_live_fixture(&self, fragment: AgentLiveFragment) {
        self.live.publish(Some(fragment));
    }

    #[cfg(feature = "test-support")]
    pub fn live_subscriber_count(&self, id: &AgentId) -> usize {
        self.live.subscriber_count(id)
    }

    pub fn install_live_source(&self, source: Arc<dyn AgentLiveSource>) {
        assert!(
            self.live_source.set(source).is_ok(),
            "live source installed twice"
        );
    }

    pub fn subscribe_local(&self, id: &AgentId) -> AgentLiveSubscription {
        self.live.subscribe(id)
    }

    pub fn subscribe_local_head(&self, id: &AgentId) -> AgentHeadSubscription {
        self.live.subscribe_head(id)
    }

    /// A peer hint wakes the existing durable reader; it carries no transcript
    /// content and cannot replace that reader's current authorization checks.
    pub fn notify_owned_head(
        &self,
        id: &AgentId,
        authority: &crate::agent_ownership::Authority,
    ) -> bool {
        if authority.token.agent_id != *id || !authority.is_current() {
            return false;
        }
        self.live.publish_head(id);
        true
    }
}
