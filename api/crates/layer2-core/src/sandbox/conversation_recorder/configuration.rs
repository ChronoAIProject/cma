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

    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_writer_policy(store, Arc::default())
    }

    pub fn with_writer_policy<S>(
        store: Arc<S>,
        writer_policy: Arc<crate::sandbox::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Arc::new(Self {
            store,
            writer_policy,
            live: live::AgentLiveHub::default(),
            live_source: std::sync::OnceLock::new(),
        })
    }
}
