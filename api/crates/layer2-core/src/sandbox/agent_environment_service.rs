use super::*;

impl AgentFileService {
    pub async fn environment(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
    ) -> Result<crate::sandbox::agent_environment::AgentEnvironment, AgentFileError> {
        use crate::sandbox::agent_environment::{
            AgentEnvironment, EnvironmentAvailability as State, EnvironmentFeature,
        };
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ViewDetails,
                AccessPurpose::Read,
            )
            .await?;
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let (state, reason) = if !authorized.decision.allows(ResourceCapability::ViewFiles) {
            (State::Forbidden, "view_files_required")
        } else if sandbox.status != SandboxStatus::Active {
            (State::NotEnabled, "runtime_not_active")
        } else {
            match runtime_request(sandbox, agent_id, "", false, |sandbox, _| {
                generation(sandbox)
            }) {
                Ok(_) => (State::Available, "authorized_mounted_home"),
                Err(AgentFileError::Unsupported) => {
                    (State::Unsupported, "mounted_home_unsupported")
                }
                Err(_) => (State::TemporarilyUnavailable, "runtime_not_ready"),
            }
        };
        let writable = matches!(state, State::Available)
            && require_home_mutation(&authorized, &caller.subject).is_ok();
        let history = authorized.decision.allows(ResourceCapability::ViewHistory);
        let browser = self.browser_feature(&authorized, agent_id).await?;
        let control = self
            .browser_control_feature(&authorized, agent_id, &browser)
            .await?;
        Ok(AgentEnvironment {
            agent_id: agent_id.to_string(),
            policy_revision: authorized.snapshot.fence().policy_revision,
            runtime_incarnation: AgentEnvironment::incarnation(agent_id, sandbox),
            revalidate_at: authorized.decision.revalidate_at.map(crate::rfc3339_at),
            files: EnvironmentFeature {
                state,
                reason,
                scope: "mounted_home",
                writable,
            },
            operation_output: EnvironmentFeature::history(history, "recorded_output_only"),
            browser,
            replay: EnvironmentFeature::history(history, "operation_records_only"),
            control,
        })
    }
}
