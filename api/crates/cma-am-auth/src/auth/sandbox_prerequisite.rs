use std::time::SystemTime;

use layer2_core::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};
use layer2_core::SubjectId;

use super::{refresh, AuthState};

#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for AuthState {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        if !self.requires_credential() {
            return Err(unavailable());
        }
        let Some(mut state) = self.store.subject(owner).await.map_err(|_| unavailable())? else {
            return Err(SandboxCredentialPrerequisiteError::SetupRequired);
        };
        if state.subject != *owner {
            return Err(unavailable());
        }
        if !has_refresh_authority(&state) {
            return Err(SandboxCredentialPrerequisiteError::SetupRequired);
        }
        refresh::refresh_if_needed(self, &mut state)
            .await
            .map_err(|_| unavailable())?;
        if !has_refresh_authority(&state) {
            return Err(SandboxCredentialPrerequisiteError::SetupRequired);
        }
        let tokens = state
            .tokens
            .as_ref()
            .expect("refresh authority requires tokens");
        if tokens.access_token.expose().trim().is_empty() || tokens.expires_at <= SystemTime::now()
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

fn has_refresh_authority(state: &super::persistence::SubjectAuthState) -> bool {
    state.tokens.as_ref().is_some_and(|tokens| {
        tokens
            .refresh_token
            .as_ref()
            .is_some_and(|token| !token.expose().trim().is_empty())
    })
}

fn unavailable() -> SandboxCredentialPrerequisiteError {
    SandboxCredentialPrerequisiteError::Unavailable(
        "renewable Sandbox credential authority could not be verified".into(),
    )
}

#[cfg(test)]
#[path = "sandbox_prerequisite_tests.rs"]
mod tests;
