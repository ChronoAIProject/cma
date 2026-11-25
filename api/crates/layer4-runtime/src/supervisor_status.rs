use layer3_adapter::{RuntimeStatus, RuntimeStatusRequest};

use super::{Phase, Supervisor};
use crate::failure::{Result, RuntimeError};

impl Supervisor {
    pub(crate) fn supports_exact_github_credential_route(&self) -> bool {
        self.config.github_proxy.is_some()
    }

    pub(crate) fn startup_policy_observation(
        &self,
        runtime: RuntimeStatus,
    ) -> layer3_adapter::RuntimeStartupPolicyObservation {
        layer3_adapter::RuntimeStartupPolicyObservation {
            runtime,
            legacy_journal_recovery_enabled: self.config.legacy_journal_recovery_enabled,
        }
    }

    pub(crate) async fn ledger_observation(
        &self,
        runtime: RuntimeStatus,
    ) -> Result<layer3_adapter::RuntimeLedgerObservation> {
        let request = RuntimeStatusRequest {
            sandbox_id: self.config.sandbox_id.clone(),
            generation: self.config.generation,
            home_identity: self.config.home_identity.clone().ok_or_else(invalid)?,
        };
        let observation = layer3_adapter::RuntimeLedgerObservation {
            runtime,
            ledger: self.mutations.observation().await?,
        };
        observation.validate_for(&request).map_err(|_| invalid())?;
        Ok(observation)
    }

    pub(super) async fn tool_proxy_status(
        &self,
        request: layer3_adapter::ToolProxyRequest,
    ) -> Result<layer3_adapter::ToolProxyStatus> {
        let invalid = || {
            RuntimeError::rejected(
                "tool_proxy_unavailable",
                "runtime cannot prove the requested tool endpoint",
            )
        };
        request.validate().map_err(|_| invalid())?;
        if request.sandbox_id != self.config.sandbox_id
            || request.generation != self.config.generation
            || self
                .config
                .tool_proxy
                .as_ref()
                .map(|config| config.upstream.as_str())
                != Some(request.endpoint.as_str())
            || self.credential.tool_proxy_generation().await != Some(request.credential_generation)
        {
            return Err(invalid());
        }
        let status = layer3_adapter::ToolProxyStatus {
            request,
            boot_id: self.boot_id.clone(),
        };
        status
            .validate_for(&status.request)
            .map_err(|_| invalid())?;
        Ok(status)
    }

    pub(super) async fn runtime_status(
        &self,
        request: RuntimeStatusRequest,
    ) -> Result<RuntimeStatus> {
        request.validate().map_err(|_| invalid())?;
        if request.sandbox_id != self.config.sandbox_id
            || request.generation != self.config.generation
            || self.config.home_identity.as_ref() != Some(&request.home_identity)
        {
            return Err(invalid());
        }
        let phase = self.phase.read().await;
        let home = crate::persistent_home::verify(&self.config)?.ok_or_else(invalid)?;
        let installation = crate::install_state::inspect(&self.config)?;
        let durable_ready = home.installation_ready(installation.as_ref())?;
        let installation_ready = matches!(*phase, Phase::Installed | Phase::Credentialed);
        if installation_ready && !durable_ready {
            return Err(RuntimeError::unavailable(
                "runtime_status_inconsistent",
                "runtime readiness is not supported by durable home state",
            ));
        }
        let status = RuntimeStatus {
            agent_browser: false,
            agent_filesystem: crate::agent_attachment::supported(&self.config),
            agent_attachment_upload: crate::agent_attachment::supported(&self.config),
            agent_skill_install: true,
            agent_skill_upgrade: false,
            agent_model_catalog: true,
            // Advertised only by the negotiated HTTP status response. Embedded
            // status in legacy Workspace protocols must retain its original shape.
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
            sandbox_id: request.sandbox_id.clone(),
            generation: request.generation,
            home_identity: request.home_identity.clone(),
            boot_id: self.boot_id.clone(),
            installation_ready,
            credential_ready: matches!(*phase, Phase::Credentialed | Phase::RestoreCredentialed),
            restore_pending: matches!(*phase, Phase::RestorePending | Phase::RestoreCredentialed),
            agent_processes: self
                .agents
                .read()
                .await
                .values()
                .filter(|agent| !agent.bridge.is_ended())
                .count()
                .try_into()
                .map_err(|_| invalid())?,
        };
        status.validate_for(&request).map_err(|_| invalid())?;
        Ok(status)
    }
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "runtime_status_identity_invalid",
        "runtime status identity does not match this persistent runtime",
    )
}
