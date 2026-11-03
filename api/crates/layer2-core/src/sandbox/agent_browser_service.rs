use super::*;
use crate::sandbox::agent_environment::{
    BrowserObservationView, BrowserSelection, EnvironmentAvailability, EnvironmentFeature,
};
use layer3_adapter::{
    AgentSlot, BrowserObservation, BrowserOperation, BrowserRequest, BrowserState,
};
use sha2::{Digest, Sha256};

impl AgentFileService {
    pub async fn browser_observation(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        selection: BrowserSelection,
    ) -> Result<BrowserObservationView, AgentFileError> {
        let BrowserSelection {
            runtime_incarnation: expected_incarnation,
            observer,
            context_id,
            enabled,
            release,
        } = selection;
        let observer = observer.as_deref();
        let context_id = context_id.as_deref();
        let capability = if enabled.is_some() {
            ResourceCapability::ManageSharing
        } else {
            ResourceCapability::ViewHistory
        };
        let purpose = if enabled.is_some() {
            AccessPurpose::Mutation {
                started_at: SystemTime::now(),
            }
        } else {
            AccessPurpose::Read
        };
        let authorized = self
            .authorize(caller, context, credential, agent_id, capability, purpose)
            .await?;
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let incarnation = incarnation(sandbox, agent_id).ok_or(AgentFileError::NotReady)?;
        if incarnation != expected_incarnation {
            return Err(AgentFileError::Conflict);
        }
        if observer.is_some_and(|value| {
            value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) || context_id.is_some_and(|value| uuid::Uuid::parse_str(value).is_err())
        {
            return Err(AgentFileError::Invalid);
        }
        let operation = match enabled {
            Some(true) => BrowserOperation::Enable,
            Some(false) => BrowserOperation::Disable,
            None if release => BrowserOperation::Release,
            None if observer.is_some() => BrowserOperation::Observe,
            None => BrowserOperation::Status,
        };
        if sandbox.status != SandboxStatus::Active || sandbox.lifecycle.operation.is_some() {
            return Ok(BrowserObservationView {
                agent_id: agent_id.to_string(),
                runtime_incarnation: incarnation,
                observation: BrowserObservation::idle(BrowserState::Paused),
            });
        }
        let scoped_observer = observer.map(|value| {
            let identity = format!(
                "{}\0{}\0{}",
                caller.subject,
                context
                    .map(|value| value.credential.credential_id.as_str())
                    .unwrap_or("personal"),
                value
            );
            format!("{:x}", Sha256::digest(identity))
        });
        let observation = self
            .read_browser(
                &authorized,
                agent_id,
                operation,
                scoped_observer,
                context_id.map(str::to_owned),
            )
            .await?;
        self.finish(&authorized, agent_id, capability).await?;
        Ok(BrowserObservationView {
            agent_id: agent_id.to_string(),
            runtime_incarnation: incarnation,
            observation,
        })
    }

    pub(super) async fn browser_feature(
        &self,
        authorized: &AuthorizedResource,
        agent_id: &AgentId,
    ) -> Result<EnvironmentFeature, AgentFileError> {
        let feature = |state, reason| EnvironmentFeature {
            state,
            reason,
            scope: "agent_browser",
            writable: false,
        };
        if !authorized.decision.allows(ResourceCapability::ViewHistory) {
            return Ok(feature(
                EnvironmentAvailability::Forbidden,
                "view_history_required",
            ));
        }
        if self.adapter.browser().is_none() {
            return Ok(EnvironmentFeature::unsupported());
        }
        if authorized.snapshot.lineage.sandbox.status != SandboxStatus::Active {
            return Ok(feature(
                EnvironmentAvailability::TemporarilyUnavailable,
                "browser_paused",
            ));
        }
        let observed = self
            .read_browser(authorized, agent_id, BrowserOperation::Status, None, None)
            .await;
        self.finish(authorized, agent_id, ResourceCapability::ViewDetails)
            .await?;
        Ok(match observed {
            Ok(value) => match value.state {
                BrowserState::Live | BrowserState::Loading | BrowserState::Ready => {
                    feature(EnvironmentAvailability::Available, "enabled_agent_browser")
                }
                BrowserState::NotEnabled => feature(
                    EnvironmentAvailability::NotEnabled,
                    "explicit_enablement_required",
                ),
                BrowserState::Unsupported => EnvironmentFeature::unsupported(),
                BrowserState::Paused => feature(
                    EnvironmentAvailability::TemporarilyUnavailable,
                    "browser_paused",
                ),
                BrowserState::Disconnected => feature(
                    EnvironmentAvailability::TemporarilyUnavailable,
                    "browser_disconnected",
                ),
            },
            Err(AgentFileError::Unsupported) => EnvironmentFeature::unsupported(),
            Err(_) => feature(
                EnvironmentAvailability::TemporarilyUnavailable,
                "browser_disconnected",
            ),
        })
    }

    async fn read_browser(
        &self,
        authorized: &AuthorizedResource,
        agent_id: &AgentId,
        operation: BrowserOperation,
        observer: Option<String>,
        context_id: Option<String>,
    ) -> Result<BrowserObservation, AgentFileError> {
        let Some(adapter) = self.adapter.browser() else {
            return Ok(BrowserObservation::idle(BrowserState::Unsupported));
        };
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let binding = sandbox.runtime.as_ref().ok_or(AgentFileError::NotReady)?;
        let request = BrowserRequest {
            sandbox_id: sandbox.id.to_string(),
            runtime_generation: generation(sandbox)?,
            boot_id: binding.boot_id.clone().ok_or(AgentFileError::NotReady)?,
            agent_id: AgentSlot(agent_id.to_string()),
            operation,
            observer,
            context_id,
        };
        let receipt = adapter
            .browser(&RuntimeHandle(runtime(sandbox)?), &request)
            .await
            .map_err(|error| match error {
                AdapterError::Unsupported(_) | AdapterError::RuntimeContractMismatch(_) => {
                    AgentFileError::Unsupported
                }
                AdapterError::Rejected(_) => AgentFileError::Conflict,
                _ => AgentFileError::Unavailable,
            })?;
        if receipt.agent_id != request.agent_id
            || receipt.boot_id != request.boot_id
            || !receipt.observation.valid()
            || request.context_id.as_ref().is_some_and(|value| {
                receipt.observation.context_id.as_ref() != Some(value)
                    && !(receipt.observation.frame.is_none()
                        && matches!(
                            receipt.observation.state,
                            BrowserState::Paused
                                | BrowserState::NotEnabled
                                | BrowserState::Unsupported
                        ))
            })
        {
            return Err(AgentFileError::Conflict);
        }
        Ok(receipt.observation)
    }
}

fn incarnation(sandbox: &Sandbox, agent_id: &AgentId) -> Option<String> {
    let binding = sandbox.runtime.as_ref()?;
    let mut digest = Sha256::new();
    digest.update(b"cma-agent-environment-v1\0");
    digest.update(agent_id.as_str());
    digest.update([0]);
    digest.update(binding.provision_generation?.to_be_bytes());
    digest.update(binding.boot_id.as_ref()?);
    Some(format!("{:x}", digest.finalize()))
}
