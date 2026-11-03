use super::*;
use crate::sandbox::{
    agent_browser_control::*,
    agent_environment::{AgentEnvironment, EnvironmentAvailability, EnvironmentFeature},
};
use layer3_adapter::{AgentSlot, BrowserControlReceipt, BrowserControlRequest};
use sha2::{Digest, Sha256};

impl AgentFileService {
    pub async fn browser_control(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        selection: BrowserControlSelection,
    ) -> Result<BrowserControlView, AgentFileError> {
        let mutation = selection.command != BrowserControlCommand::Status;
        let capability = if mutation || selection.operation_id.is_some() {
            ResourceCapability::ManageSharing
        } else {
            ResourceCapability::ViewHistory
        };
        let purpose = if mutation {
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
        let incarnation =
            AgentEnvironment::incarnation(agent_id, sandbox).ok_or(AgentFileError::NotReady)?;
        if incarnation != selection.runtime_incarnation {
            return Err(AgentFileError::Conflict);
        }
        if (mutation || selection.operation_id.is_some())
            && selection
                .client_key
                .as_ref()
                .is_none_or(|value| uuid::Uuid::parse_str(value).is_err())
        {
            return Err(AgentFileError::Invalid);
        }
        let actor = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}\0{}\0{}",
                caller.subject,
                context
                    .map(|value| value.credential.credential_id.as_str())
                    .unwrap_or("personal"),
                selection.client_key.as_deref().unwrap_or_default()
            ))
        );
        let request = control_request(sandbox, agent_id, actor, selection)?;
        if !request.valid() {
            return Err(AgentFileError::Invalid);
        }
        self.finish(&authorized, agent_id, capability).await?;
        let receipt = self.send_browser_control(&authorized, &request).await?;
        self.finish(&authorized, agent_id, capability).await?;
        Ok(BrowserControlView {
            agent_id: agent_id.to_string(),
            runtime_incarnation: incarnation,
            context_id: receipt.context_id,
            control: receipt.control,
        })
    }

    pub(super) async fn browser_control_feature(
        &self,
        authorized: &AuthorizedResource,
        agent_id: &AgentId,
        browser: &EnvironmentFeature,
    ) -> Result<EnvironmentFeature, AgentFileError> {
        let feature = |state, reason, writable| EnvironmentFeature {
            state,
            reason,
            scope: "agent_browser",
            writable,
        };
        if !authorized
            .decision
            .allows(ResourceCapability::ManageSharing)
        {
            return Ok(feature(
                EnvironmentAvailability::Forbidden,
                "browser_owner_required",
                false,
            ));
        }
        if self.adapter.browser().is_none() {
            return Ok(EnvironmentFeature::unsupported());
        }
        if !matches!(browser.state, EnvironmentAvailability::Available) {
            return Ok(feature(
                EnvironmentAvailability::NotEnabled,
                "browser_not_available",
                false,
            ));
        }
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let mut request = control_request(
            sandbox,
            agent_id,
            "capability".into(),
            BrowserControlSelection {
                runtime_incarnation: String::new(),
                context_id: String::new(),
                client_id: "capability".into(),
                client_key: None,
                command: BrowserControlCommand::Status,
                operation_id: None,
                expected_revision: None,
                lease_token: None,
                input: None,
            },
        )?;
        request.context_id = None;
        let result = self.send_browser_control(authorized, &request).await;
        self.finish(authorized, agent_id, ResourceCapability::ViewDetails)
            .await?;
        Ok(match result {
            Ok(value) if value.control.state != BrowserControlState::Unknown => feature(
                EnvironmentAvailability::Available,
                "exclusive_browser_control",
                true,
            ),
            Err(AgentFileError::Unsupported) => EnvironmentFeature::unsupported(),
            _ => feature(
                EnvironmentAvailability::TemporarilyUnavailable,
                "browser_control_unknown",
                false,
            ),
        })
    }

    async fn send_browser_control(
        &self,
        authorized: &AuthorizedResource,
        request: &BrowserControlRequest,
    ) -> Result<BrowserControlReceipt, AgentFileError> {
        let sandbox = &authorized.snapshot.lineage.sandbox;
        if sandbox.status != SandboxStatus::Active || sandbox.lifecycle.operation.is_some() {
            return Err(AgentFileError::NotReady);
        }
        let adapter = self.adapter.browser().ok_or(AgentFileError::Unsupported)?;
        let receipt = adapter
            .browser_control(&RuntimeHandle(runtime(sandbox)?), request)
            .await
            .map_err(|error| match error {
                AdapterError::Unsupported(_) | AdapterError::RuntimeContractMismatch(_) => {
                    AgentFileError::Unsupported
                }
                AdapterError::Rejected(_) | AdapterError::OperationIdentityConflict(_) => {
                    AgentFileError::Conflict
                }
                _ => AgentFileError::Unavailable,
            })?;
        if receipt.agent_id != request.agent_id
            || receipt.boot_id != request.boot_id
            || request
                .context_id
                .as_ref()
                .is_some_and(|value| *value != receipt.context_id)
            || receipt.control.operation_id != request.operation_id
            || !receipt.control.valid()
            || uuid::Uuid::parse_str(&receipt.context_id).is_err()
        {
            return Err(AgentFileError::Conflict);
        }
        Ok(receipt)
    }
}

fn control_request(
    sandbox: &Sandbox,
    agent_id: &AgentId,
    actor: String,
    selection: BrowserControlSelection,
) -> Result<BrowserControlRequest, AgentFileError> {
    let binding = sandbox.runtime.as_ref().ok_or(AgentFileError::NotReady)?;
    Ok(BrowserControlRequest {
        sandbox_id: sandbox.id.to_string(),
        runtime_generation: generation(sandbox)?,
        boot_id: binding.boot_id.clone().ok_or(AgentFileError::NotReady)?,
        agent_id: AgentSlot(agent_id.to_string()),
        context_id: Some(selection.context_id),
        actor,
        client_id: selection.client_id,
        command: selection.command,
        operation_id: selection.operation_id,
        expected_revision: selection.expected_revision,
        lease_token: selection.lease_token,
        input: selection.input,
    })
}
