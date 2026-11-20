use super::AgentReadService;
use crate::sandbox::store::SandboxStoreError;
use crate::SandboxId;

impl AgentReadService {
    pub(in crate::sandbox) fn session_store(
        &self,
    ) -> Result<std::sync::Arc<dyn super::super::store::SandboxSessionStore>, SandboxStoreError>
    {
        self.sessions
            .clone()
            .ok_or_else(|| SandboxStoreError::Unavailable("session store is unavailable".into()))
    }

    pub(crate) async fn renew_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError> {
        self.sessions
            .as_ref()
            .ok_or(SandboxStoreError::Unavailable(
                "session store is unavailable".into(),
            ))?
            .renew_sandbox_session(sandbox_id, session_id, lease_expires_at)
            .await
    }

    pub(crate) async fn close_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
    ) -> Result<(), SandboxStoreError> {
        self.sessions
            .as_ref()
            .ok_or(SandboxStoreError::Unavailable(
                "session store is unavailable".into(),
            ))?
            .close_sandbox_session(sandbox_id, session_id)
            .await
    }
}
