use std::sync::Arc;

use layer3_adapter::{
    AgentSlot, BrowserOperation, BrowserRequest, BrowserState, RuntimeHandle, SandboxAdapter,
    SandboxCapacity,
};
use serde::{Deserialize, Serialize};

use super::{Agent, Sandbox, SandboxStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopAvailability {
    Available,
    Unsupported,
    TemporarilyUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopCapability {
    pub state: DesktopAvailability,
    pub reason: String,
    pub runtime_contract: u32,
    pub min_memory_mib: u32,
    pub min_cpu_millis: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_image: Option<String>,
}

impl DesktopCapability {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            state: DesktopAvailability::Unsupported,
            reason: reason.into(),
            runtime_contract: 3,
            min_memory_mib: 2048,
            min_cpu_millis: 1000,
            runtime_image: None,
        }
    }

    pub fn available(&self) -> bool {
        self.state == DesktopAvailability::Available
    }
}

pub struct DesktopCreationService {
    adapter: Arc<dyn SandboxAdapter>,
    released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    verified_image: Option<String>,
}

impl DesktopCreationService {
    pub fn new(
        adapter: Arc<dyn SandboxAdapter>,
        default_image: String,
        verified_image: Option<String>,
    ) -> Result<Arc<Self>, &'static str> {
        Self::with_released(
            adapter,
            Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
                default_image,
                None,
            )),
            verified_image,
        )
    }

    /// The default image is the source's agent-lane release at each check.
    pub fn with_released(
        adapter: Arc<dyn SandboxAdapter>,
        released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
        verified_image: Option<String>,
    ) -> Result<Arc<Self>, &'static str> {
        if verified_image.as_ref().is_some_and(|image| {
            image.rsplit_once("@sha256:").is_none_or(|(name, digest)| {
                name.is_empty()
                    || digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        }) {
            return Err("desktop verification requires an immutable image digest");
        }
        Ok(Arc::new(Self {
            adapter,
            released,
            verified_image,
        }))
    }

    pub async fn capability(
        &self,
        selected_image: Option<&str>,
        capacity: SandboxCapacity,
        parent: Option<&Sandbox>,
    ) -> DesktopCapability {
        self.capability_for_verified(
            self.verified_image.as_deref(),
            selected_image,
            capacity,
            parent,
        )
        .await
    }

    /// Check a fresh desktop canary using its separately verified catalog image.
    pub async fn canary_capability(
        &self,
        verified_image: &str,
        capacity: SandboxCapacity,
    ) -> DesktopCapability {
        self.capability_for_verified(Some(verified_image), Some(verified_image), capacity, None)
            .await
    }

    async fn capability_for_verified(
        &self,
        verified_image: Option<&str>,
        selected_image: Option<&str>,
        capacity: SandboxCapacity,
        parent: Option<&Sandbox>,
    ) -> DesktopCapability {
        let mut result =
            DesktopCapability::unavailable("This deployment has no verified desktop runtime.");
        let Some(verified) = verified_image else {
            return result;
        };
        if self.adapter.browser().is_none() {
            result.reason = "This runtime adapter does not support an Agent browser.".into();
            return result;
        }
        if self.adapter.home_volumes().is_none() {
            result.reason =
                "This deployment cannot retain the desktop runtime's home identity.".into();
            return result;
        }
        let default_image = self.released.snapshot().agent;
        let image = parent
            .and_then(|sandbox| {
                sandbox
                    .runtime
                    .as_ref()
                    .map(|runtime| runtime.runtime_version.as_str())
            })
            .or(selected_image)
            .unwrap_or(&default_image);
        if image != verified {
            result.reason = "This image is not verified for desktop creation. Existing Sandboxes require an explicit upgrade; they will not restart automatically.".into();
            return result;
        }
        let capacity = parent
            .map(|sandbox| SandboxCapacity {
                cpu_millis: sandbox.profile.content.cpu_millis,
                memory_mib: sandbox.profile.content.memory_mib,
            })
            .unwrap_or(capacity);
        if capacity.cpu_millis < result.min_cpu_millis
            || capacity.memory_mib < result.min_memory_mib
        {
            result.reason = "Desktop creation requires at least 1000 millicores and 2048 MiB of configured capacity.".into();
            return result;
        }
        if let Some(sandbox) = parent {
            if sandbox.status != SandboxStatus::Active || sandbox.lifecycle.operation.is_some() {
                result.reason = "The selected Sandbox must already be active. Desktop creation will not restart it.".into();
                return result;
            }
            let Some(binding) = &sandbox.runtime else {
                return result;
            };
            match self
                .adapter
                .runtime_status(&RuntimeHandle(binding.runtime_id.clone()))
                .await
            {
                Ok(status)
                    if status.sandbox_id == sandbox.id.as_str()
                        && Some(status.generation) == binding.provision_generation
                        && Some(status.boot_id.as_str()) == binding.boot_id.as_deref()
                        && status.credential_ready
                        && !status.restore_pending =>
                {
                    if !status.agent_browser {
                        result.reason = "The retained runtime does not support an Agent browser; an explicit upgrade is required.".into();
                        return result;
                    }
                }
                _ => {
                    result.state = DesktopAvailability::TemporarilyUnavailable;
                    result.reason = "The selected runtime capability could not be verified. Retry the same request.".into();
                    return result;
                }
            }
        }
        result.state = DesktopAvailability::Available;
        result.reason = "Verified runtime; desktop preparation precedes the first task.".into();
        result.runtime_image = Some(image.into());
        result
    }
}

pub async fn prepare_desktop(
    adapter: &dyn SandboxAdapter,
    agent: &Agent,
    sandbox: &Sandbox,
) -> Result<(), &'static str> {
    if !agent
        .conversation
        .first_message
        .as_ref()
        .is_some_and(|message| message.desktop)
    {
        return Ok(());
    }
    let unavailable =
        "Desktop preparation is unavailable. The original Agent and task are retained for retry.";
    if sandbox.status != SandboxStatus::Active
        || sandbox.lifecycle.operation.is_some()
        || agent.sandbox_id != sandbox.id
    {
        return Err(unavailable);
    }
    let binding = sandbox.runtime.as_ref().ok_or(unavailable)?;
    let request = BrowserRequest {
        sandbox_id: sandbox.id.to_string(),
        runtime_generation: binding.provision_generation.ok_or(unavailable)?,
        boot_id: binding.boot_id.clone().ok_or(unavailable)?,
        agent_id: AgentSlot(agent.id.to_string()),
        operation: BrowserOperation::Enable,
        observer: None,
        context_id: None,
    };
    let receipt = adapter
        .browser()
        .ok_or(unavailable)?
        .browser(&RuntimeHandle(binding.runtime_id.clone()), &request)
        .await
        .map_err(|_| unavailable)?;
    if receipt.agent_id != request.agent_id
        || receipt.boot_id != request.boot_id
        || !receipt.observation.valid()
        || receipt.observation.context_id.is_none()
        || !matches!(
            receipt.observation.state,
            BrowserState::Loading | BrowserState::Ready | BrowserState::Live
        )
    {
        return Err(unavailable);
    }
    Ok(())
}
