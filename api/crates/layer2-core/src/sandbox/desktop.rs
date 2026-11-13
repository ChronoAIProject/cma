//! Owner-authorized access to the desktop of a retained Sandbox.
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use layer3_adapter::{DesktopConnectionHandle, RuntimeHandle, SandboxAdapter};
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::{
    register_open_desktop_session, unregister_open_desktop_session, AgentReadService, AgentStatus,
    SandboxStatus,
};
use crate::ids::SandboxCommandId;
use crate::{rfc3339_at, AgentId, Caller, Capability, SandboxId};

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
    sandbox_id: SandboxId,
    session_id: String,
    runtime_id: String,
    _lease: OwnedMutexGuard<()>,
}

impl Drop for DesktopConnection {
    fn drop(&mut self) {
        unregister_open_desktop_session(&self.sandbox_id);
    }
}

impl DesktopConnection {
    pub fn session_ids(&self) -> (SandboxId, String) {
        (self.sandbox_id.clone(), self.session_id.clone())
    }

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
                AgentStatus::Starting
                    | AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
            )
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
        let Ok((sandbox_id, runtime)) = self.authorize(caller, id).await else {
            return false;
        };
        if runtime != connection.runtime_id {
            return false;
        }
        self.reads
            .renew_sandbox_session(
                &SandboxId(sandbox_id),
                &connection.session_id,
                &session_expiry(),
            )
            .await
            .is_ok()
    }

    pub async fn close(&self, sandbox_id: SandboxId, session_id: String) {
        let _ = self
            .reads
            .close_sandbox_session(&sandbox_id, &session_id)
            .await;
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
            leases.insert(sandbox_id.clone(), Arc::downgrade(&lease));
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
        let sandbox_id = SandboxId(sandbox_id);
        let session_id = SandboxCommandId::generate().to_string();
        self.reads
            .open_sandbox_session(&sandbox_id, &session_id, &session_expiry())
            .await
            .map_err(|_| DesktopError::Unavailable)?;
        register_open_desktop_session(&sandbox_id);
        Ok(DesktopConnection {
            connection,
            sandbox_id,
            session_id,
            runtime_id,
            _lease: lease,
        })
    }
}

fn session_expiry() -> String {
    rfc3339_at(SystemTime::now() + Duration::from_secs(90))
}
