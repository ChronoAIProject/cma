//! Owner-authorized access to the desktop of a retained Sandbox.
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use layer3_adapter::{DesktopConnectionHandle, RuntimeHandle, SandboxAdapter};
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::{AgentReadService, AgentStatus, SandboxStatus};
use crate::{AgentId, Caller, Capability, RuntimeCapability};

pub use layer3_adapter::DesktopFrame;

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("desktop is unavailable for this Agent")]
    Unavailable,
    #[error("this Sandbox desktop is already connected")]
    Busy,
    #[error("desktop runtime could not be reached")]
    RuntimeUnavailable,
}

pub struct DesktopService {
    reads: Arc<AgentReadService>,
    adapter: Arc<dyn SandboxAdapter>,
    leases: Mutex<BTreeMap<String, Weak<Mutex<()>>>>,
}

pub struct DesktopConnection {
    connection: DesktopConnectionHandle,
    runtime_id: String,
    _lease: OwnedMutexGuard<()>,
}

impl DesktopConnection {
    pub async fn send(&mut self, frame: DesktopFrame) -> Result<(), DesktopError> {
        self.connection
            .send(frame)
            .await
            .map_err(|_| DesktopError::RuntimeUnavailable)
    }

    pub async fn recv(&mut self) -> Result<Option<DesktopFrame>, DesktopError> {
        self.connection
            .recv()
            .await
            .map_err(|_| DesktopError::RuntimeUnavailable)
    }
}

impl DesktopService {
    pub fn new(reads: Arc<AgentReadService>, adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            reads,
            adapter,
            leases: Mutex::new(BTreeMap::new()),
        }
    }

    async fn authorize(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<(String, String), DesktopError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(DesktopError::Unavailable);
        }
        let lineage = self
            .reads
            .lineage(caller, id)
            .await
            .map_err(|_| DesktopError::Unavailable)?;
        if lineage.sandbox.status != SandboxStatus::Active
            || !matches!(
                lineage.agent.status,
                AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
            )
            || !lineage
                .agent
                .capabilities
                .supports(RuntimeCapability::Desktop)
            || lineage.agent.profile.content.isolated_agent
            || !lineage
                .sandbox
                .profile
                .content
                .environment_variables
                .iter()
                .any(|variable| variable.name == "CMA_MACHINE_INCLUDED" && variable.value == "1")
        {
            return Err(DesktopError::Unavailable);
        }
        let runtime = lineage
            .sandbox
            .runtime
            .filter(|runtime| !runtime.runtime_id.trim().is_empty())
            .ok_or(DesktopError::Unavailable)?;
        Ok((lineage.sandbox.id.to_string(), runtime.runtime_id))
    }

    pub async fn available(&self, caller: &Caller, id: &AgentId) -> Result<(), DesktopError> {
        self.authorize(caller, id).await.map(|_| ())
    }

    pub async fn still_authorized(
        &self,
        caller: &Caller,
        id: &AgentId,
        connection: &mut DesktopConnection,
    ) -> bool {
        self.authorize(caller, id)
            .await
            .is_ok_and(|(_, runtime)| runtime == connection.runtime_id)
    }

    pub async fn open(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<DesktopConnection, DesktopError> {
        let (sandbox_id, runtime_id) = self.authorize(caller, id).await?;
        let lease = {
            let mut leases = self.leases.lock().await;
            leases.retain(|_, value| value.strong_count() > 0);
            let lease = leases
                .get(&sandbox_id)
                .and_then(Weak::upgrade)
                .unwrap_or_default();
            leases.insert(sandbox_id, Arc::downgrade(&lease));
            lease.try_lock_owned().map_err(|_| DesktopError::Busy)?
        };
        let connection = tokio::time::timeout(
            Duration::from_secs(10),
            self.adapter
                .open_desktop(&RuntimeHandle::new(runtime_id.clone())),
        )
        .await
        .map_err(|_| DesktopError::RuntimeUnavailable)?
        .map_err(|_| DesktopError::RuntimeUnavailable)?;
        Ok(DesktopConnection {
            connection,
            runtime_id,
            _lease: lease,
        })
    }
}
