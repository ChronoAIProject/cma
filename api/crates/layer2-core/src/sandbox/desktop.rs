//! Owner-authorized access to the desktop of a retained Sandbox.
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer3_adapter::{DesktopConnectionHandle, RuntimeHandle, SandboxAdapter};
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::{
    unregister_open_desktop_session, AgentReadService, AgentStatus, RuntimeBinding, SandboxStatus,
};
use crate::ids::SandboxCommandId;
use crate::{rfc3339_at, AgentId, Caller, Capability, SandboxId};

pub use layer3_adapter::DesktopFrame;

mod authorization;
mod owned;
mod timing;
use super::runtime_migration_guard::{try_acquire_desktop_runtime_guard, DesktopRuntimeGuard};
pub use authorization::DesktopAuthorization;

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error(transparent)]
    RuntimeCapacity(super::RuntimeCapacityError),
    #[error("desktop is unavailable for this Agent")]
    Unavailable,
    #[error("this Sandbox desktop is already connected")]
    Busy,
    #[error("desktop runtime could not be reached")]
    RuntimeUnavailable,
    #[error(
        "The desktop could not recover automatically. Contact support if you need desktop access."
    )]
    RuntimeFailed,
    #[error("desktop session authority is temporarily unavailable")]
    AuthorityUnavailable,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum DesktopAdmission {
    #[default]
    Disabled,
    Protected,
}

#[derive(Clone)]
pub struct DesktopService {
    reads: Arc<AgentReadService>,
    adapter: Arc<dyn SandboxAdapter>,
    admission: DesktopAdmission,
    migration_tracking: bool,
}

pub struct DesktopConnection {
    connection: Option<DesktopConnectionHandle>,
    sandbox_id: SandboxId,
    session_id: String,
    runtime_id: String,
    _lease: Option<OwnedMutexGuard<()>>,
    migration_guard: Option<DesktopRuntimeGuard>,
    owned: Option<owned::ConnectionState>,
}

impl Drop for DesktopConnection {
    fn drop(&mut self) {
        if self._lease.is_some() {
            unregister_open_desktop_session(&self.sandbox_id);
        }
    }
}

impl DesktopConnection {
    /// Do not keep publishing buffered frames after the connection loses its
    /// authority while the client transport is blocked by backpressure.
    pub async fn while_authorized<T>(
        &mut self,
        operation: impl std::future::Future<Output = T>,
    ) -> Result<T, DesktopError> {
        if self.connection.is_none() {
            return Err(DesktopError::Unavailable);
        }
        if let Some(owned) = &mut self.owned {
            owned.while_authorized(operation).await
        } else {
            Ok(operation.await)
        }
    }

    pub async fn send(&mut self, frame: DesktopFrame) -> Result<(), DesktopError> {
        let connection = self.connection.as_mut().ok_or(DesktopError::Unavailable)?;
        if let Some(owned) = &mut self.owned {
            return owned.send(connection, frame).await;
        }
        connection.send(frame).await.map_err(DesktopError::adapter)
    }

    pub async fn recv(&mut self) -> Result<Option<DesktopFrame>, DesktopError> {
        let connection = self.connection.as_mut().ok_or(DesktopError::Unavailable)?;
        if let Some(owned) = &mut self.owned {
            return owned.recv(connection).await;
        }
        connection.recv().await.map_err(DesktopError::adapter)
    }
}

impl DesktopService {
    pub fn new(reads: Arc<AgentReadService>, adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self {
            reads,
            adapter,
            admission: DesktopAdmission::Disabled,
            migration_tracking: false,
        }
    }

    pub fn with_admission(mut self, admission: DesktopAdmission) -> Self {
        self.admission = admission;
        self
    }

    pub fn with_runtime_migration_tracking(mut self, enabled: bool) -> Self {
        self.migration_tracking = enabled;
        self
    }

    fn require_protected(&self) -> Result<(), DesktopError> {
        match self.admission {
            DesktopAdmission::Protected => Ok(()),
            DesktopAdmission::Disabled => {
                tracing::debug!(reason = "disabled", "desktop admission refused");
                Err(DesktopError::AuthorityUnavailable)
            }
        }
    }

    async fn authorize(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<(SandboxId, RuntimeBinding), DesktopError> {
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
        Ok((lineage.sandbox.id, runtime))
    }

    pub async fn available(&self, caller: &Caller, id: &AgentId) -> Result<(), DesktopError> {
        self.require_protected()?;
        let (sandbox_id, _) = self.authorize(caller, id).await?;
        // Discovery makes the old viewer's outstanding lease observable before
        // a new WebSocket hides its refusal headers. Opening still checks atomically.
        let store = self
            .reads
            .session_store()
            .map_err(|_| DesktopError::AuthorityUnavailable)?;
        let occupied = tokio::time::timeout(
            Duration::from_secs(3),
            store.has_open_sandbox_session(&sandbox_id, &crate::now_rfc3339()),
        )
        .await
        .map_err(|_| DesktopError::AuthorityUnavailable)?
        .map_err(|_| DesktopError::AuthorityUnavailable)?;
        if occupied {
            return Err(DesktopError::Busy);
        }
        Ok(())
    }

    pub async fn still_authorized(
        &self,
        caller: &Caller,
        id: &AgentId,
        connection: &mut DesktopConnection,
    ) -> bool {
        if let Some(owned) = &connection.owned {
            // Replica-mode renewal revalidates caller/binding independently of
            // frame forwarding; this check never stalls frames on Mongo I/O.
            return owned.valid();
        }
        let Ok((sandbox_id, runtime)) = self.authorize(caller, id).await else {
            return false;
        };
        if runtime.runtime_id != connection.runtime_id {
            return false;
        }
        self.reads
            .renew_sandbox_session(&sandbox_id, &connection.session_id, &session_expiry())
            .await
            .is_ok()
    }

    pub async fn close_connection(&self, connection: &mut DesktopConnection) {
        if let Some(owned) = &mut connection.owned {
            // Receiver acknowledgement drains old input before shared release.
            // Uncertainty retains the durable claim while dropping our socket.
            owned.close(&self.reads).await;
            connection.connection.take();
            connection.migration_guard.take();
        } else {
            // Preserve the legacy handoff: slow persistence cannot retain the
            // old runtime socket or the local exclusive viewer lease.
            connection.connection.take();
            if connection._lease.take().is_some() {
                unregister_open_desktop_session(&connection.sandbox_id);
            }
            self.close(connection.sandbox_id.clone(), connection.session_id.clone())
                .await;
            connection.migration_guard.take();
        }
    }

    pub async fn close(&self, sandbox_id: SandboxId, session_id: String) {
        match tokio::time::timeout(
            Duration::from_secs(2),
            self.reads.close_sandbox_session(&sandbox_id, &session_id),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(_)) => tracing::warn!(
                "desktop session close could not be recorded; awaiting session expiry"
            ),
            Err(_) => tracing::warn!("desktop session close timed out; awaiting session expiry"),
        }
    }

    pub async fn open(
        &self,
        _caller: &Caller,
        _id: &AgentId,
    ) -> Result<DesktopConnection, DesktopError> {
        self.require_protected()?;
        // A captured Caller cannot observe logout or revoked credentials.
        tracing::debug!(
            reason = "missing_current_authority",
            "desktop admission refused"
        );
        Err(DesktopError::AuthorityUnavailable)
    }
}

fn session_expiry() -> String {
    rfc3339_at(SystemTime::now() + Duration::from_secs(90))
}

#[cfg(test)]
#[path = "desktop_lease_tests.rs"]
mod lease_tests;

impl DesktopError {
    fn adapter(error: layer3_adapter::AdapterError) -> Self {
        if matches!(error, layer3_adapter::AdapterError::DesktopFailed) {
            return Self::RuntimeFailed;
        }
        super::RuntimeCapacityError::from_adapter(&error)
            .map(Self::RuntimeCapacity)
            .unwrap_or(Self::RuntimeUnavailable)
    }
}
