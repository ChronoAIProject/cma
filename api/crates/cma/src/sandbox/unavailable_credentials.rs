//! Fail-closed Sandbox credential authority for trusted local authentication.

use std::time::SystemTime;

use layer2_core::sandbox::Sandbox;
use layer2_core::SandboxId;

use super::credential::{CredentialError, CredentialMaterial, SandboxCredentialController};

#[derive(Default)]
pub(crate) struct UnavailableSandboxCredentials;

#[async_trait::async_trait]
impl SandboxCredentialController for UnavailableSandboxCredentials {
    async fn settle_revocations(&self, _sandbox: &Sandbox) -> Result<(), CredentialError> {
        Err(unavailable())
    }

    async fn issue(
        &self,
        _sandbox: &Sandbox,
        _generation: u64,
        _now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        Err(unavailable())
    }

    async fn material(
        &self,
        _sandbox_id: &SandboxId,
        _generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        Ok(None)
    }

    async fn custodied(
        &self,
        _sandbox_id: &SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        Ok(Vec::new())
    }

    async fn forget(
        &self,
        _sandbox_id: &SandboxId,
        _generation: u64,
    ) -> Result<(), CredentialError> {
        Ok(())
    }

    async fn revoke(
        &self,
        _sandbox: &Sandbox,
        _provider_key_id: &str,
    ) -> Result<(), CredentialError> {
        Err(unavailable())
    }
}

fn unavailable() -> CredentialError {
    CredentialError::Security(
        "sandbox credentials require provider authentication authority".into(),
    )
}
