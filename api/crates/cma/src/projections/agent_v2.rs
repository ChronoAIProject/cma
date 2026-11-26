use super::*;

impl AgentProjectionServices {
    pub(crate) fn with_external_approval_review_origin(mut self, origin: &str) -> Self {
        self.external_review_origin = origin.into();
        self
    }
    pub(crate) fn command_service(
        &self,
    ) -> Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>> {
        self.commands.clone()
    }
    pub(crate) fn agent_events_router(
        &self,
        flags: crate::config::FeatureFlags,
        updates: Arc<dyn AgentUpdateRecorder>,
    ) -> Router {
        let Some(commands) = &self.commands else {
            return Router::new();
        };
        layer1_public::agent_v2::router(
            layer1_public::agent_v2::AgentV2State::new(self.reads(), commands.clone())
                .with_external_approval_review_origin(&self.external_review_origin)
                .with_task_experience(commands.task_experience_policy())
                .with_updates(updates)
                .with_event_wake(
                    flags.agent_event_stream_wake,
                    flags.agent_event_stream_fallback,
                )
                .with_single_fence_pages(flags.single_fence_event_pages)
                .with_ui_transport(
                    flags.v2_ui_transport,
                    layer1_public::agent_v2::UiTransportDependencies {
                        responses: true,
                        controls: true,
                    },
                ),
        )
    }
}
