use async_trait::async_trait;

use crate::sandbox::store::{SandboxSessionStore, SandboxStoreError};
use crate::sandbox::SandboxStatus;
use crate::{seconds_from_rfc3339, SandboxId};

use super::InMemorySandboxStore;

const MAX_SESSION_ID_BYTES: usize = 128;

#[async_trait]
impl SandboxSessionStore for InMemorySandboxStore {
    async fn open_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError> {
        validate(sandbox_id, session_id, lease_expires_at)?;
        let mut state = self.lock();
        let sandbox = state
            .sandboxes
            .get(sandbox_id)
            .ok_or(SandboxStoreError::NotFound)?;
        if sandbox.status != SandboxStatus::Active {
            return Err(SandboxStoreError::Conflict);
        }
        let key = (sandbox_id.clone(), session_id.to_owned());
        if state.sandbox_sessions.contains_key(&key) {
            return Err(SandboxStoreError::Conflict);
        }
        state
            .sandbox_sessions
            .insert(key, lease_expires_at.to_owned());
        Ok(())
    }

    async fn renew_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError> {
        validate(sandbox_id, session_id, lease_expires_at)?;
        let mut state = self.lock();
        let lease = state
            .sandbox_sessions
            .get_mut(&(sandbox_id.clone(), session_id.to_owned()))
            .ok_or(SandboxStoreError::NotFound)?;
        *lease = lease_expires_at.to_owned();
        Ok(())
    }

    async fn close_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
    ) -> Result<(), SandboxStoreError> {
        if sandbox_id.as_str().trim().is_empty()
            || session_id.trim().is_empty()
            || session_id.len() > MAX_SESSION_ID_BYTES
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox session request is invalid",
            ));
        }
        self.lock()
            .sandbox_sessions
            .remove(&(sandbox_id.clone(), session_id.to_owned()));
        Ok(())
    }

    async fn has_open_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        observed_at: &str,
    ) -> Result<bool, SandboxStoreError> {
        let observed = seconds_from_rfc3339(observed_at).ok_or(
            SandboxStoreError::InvalidTransaction("sandbox session observation is invalid"),
        )?;
        let mut state = self.lock();
        state.sandbox_sessions.retain(|_, expires_at| {
            seconds_from_rfc3339(expires_at).is_some_and(|expires| expires > observed)
        });
        Ok(state
            .sandbox_sessions
            .iter()
            .any(|((id, _), _)| id == sandbox_id))
    }
}

fn validate(
    sandbox_id: &SandboxId,
    session_id: &str,
    lease_expires_at: &str,
) -> Result<(), SandboxStoreError> {
    if sandbox_id.as_str().trim().is_empty()
        || session_id.trim().is_empty()
        || session_id.len() > MAX_SESSION_ID_BYTES
        || seconds_from_rfc3339(lease_expires_at).is_none()
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "sandbox session request is invalid",
        ));
    }
    Ok(())
}
