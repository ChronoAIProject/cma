use super::*;

impl AgentProjectionServices {
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
                .with_task_experience(commands.task_experience_policy())
                .with_updates(updates)
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
