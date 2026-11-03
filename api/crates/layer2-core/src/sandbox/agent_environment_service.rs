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
        use sha2::{Digest, Sha256};
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
        let runtime_incarnation = sandbox.runtime.as_ref().and_then(|runtime| {
            let boot = runtime.boot_id.as_ref()?;
            let generation = runtime.provision_generation?;
            let mut digest = Sha256::new();
            digest.update(b"cma-agent-environment-v1\0");
            digest.update(agent_id.as_str());
            digest.update([0]);
            digest.update(generation.to_be_bytes());
            digest.update(boot);
            Some(format!("{:x}", digest.finalize()))
        });
        Ok(AgentEnvironment {
            agent_id: agent_id.to_string(),
            policy_revision: authorized.snapshot.fence().policy_revision,
            runtime_incarnation,
            revalidate_at: authorized.decision.revalidate_at.map(crate::rfc3339_at),
            files: EnvironmentFeature {
                state,
                reason,
                scope: "mounted_home",
                writable,
            },
            operation_output: EnvironmentFeature {
                state: if history {
                    State::Available
                } else {
                    State::Forbidden
                },
                reason: if history {
                    "recorded_output_only"
                } else {
                    "view_history_required"
                },
                scope: "agent_turn_history",
                writable: false,
            },
            browser: self.browser_feature(&authorized, agent_id).await?,
            replay: EnvironmentFeature::unsupported(),
            control: EnvironmentFeature::unsupported(),
        })
    }
}
