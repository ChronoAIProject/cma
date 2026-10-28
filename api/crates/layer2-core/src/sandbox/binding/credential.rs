use crate::{CoreError, SandboxId, ServiceId, SubjectId};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Request-only secret. Retained binding records contain its hash, never bytes.
#[derive(Clone)]
pub struct ProvidedCredential(Zeroizing<String>);

impl ProvidedCredential {
    pub fn new(value: String) -> Result<Self, CoreError> {
        let value = Zeroizing::new(value);
        if value.is_empty()
            || value.len() > 4096
            || value
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(CoreError::InvalidRequest(
                "invalid supplied credential".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn sha256(&self) -> String {
        format!("{:x}", Sha256::digest(self.0.as_bytes()))
    }
}

impl std::fmt::Debug for ProvidedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProvidedCredential(<redacted>)")
    }
}

/// Store caller-supplied credential bytes using the existing encrypted custody.
/// A Sandbox's first generation is immutable: retries must present the same key.
#[async_trait::async_trait]
pub trait BindingCredentialCustody: Send + Sync {
    /// Refuse unusable credential formats before reserving a binding identifier.
    fn validate(&self, _credential: &ProvidedCredential) -> Result<(), CoreError> {
        Ok(())
    }

    /// Implementations fence the persisted binding revision in the credential
    /// transaction. A late caller must not repopulate retired custody.
    async fn retain_binding(
        &self,
        _binding: &super::ProviderBinding,
        _credential: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        Err(CoreError::RuntimeUnavailable(
            "binding credential admission is unavailable".into(),
        ))
    }

    /// Atomically reserve replacement custody and its Sandbox delivery with the
    /// current binding revision. Existing conversations keep their generation.
    async fn rotate_binding(
        &self,
        _binding: &super::ProviderBinding,
        _credential: &ProvidedCredential,
    ) -> Result<super::ProviderBinding, CoreError> {
        Err(CoreError::RuntimeUnavailable(
            "binding credential rotation is unavailable".into(),
        ))
    }

    /// Restore only the binding's readiness after its original hierarchy has
    /// recovered. Implementations atomically fence the binding, both parents and
    /// the current retained credential; they must not provision or rotate.
    async fn retry_recovered_binding(
        &self,
        _binding: &super::ProviderBinding,
        _credential: &ProvidedCredential,
    ) -> Result<super::ProviderBinding, CoreError> {
        Err(CoreError::RuntimeUnavailable(
            "binding recovery admission is unavailable".into(),
        ))
    }

    /// Continue the original durable binding intent using only its existing
    /// encrypted credential. The same recovery fences as an identical PUT apply.
    async fn recover_binding(
        &self,
        _binding: &super::ProviderBinding,
    ) -> Result<super::ProviderBinding, CoreError> {
        Err(CoreError::RuntimeUnavailable(
            "binding recovery admission is unavailable".into(),
        ))
    }

    async fn forget_binding(
        &self,
        _binding: &super::ProviderBinding,
        _sandbox_id: &SandboxId,
    ) -> Result<(), CoreError> {
        Err(CoreError::RuntimeUnavailable(
            "binding credential retirement is unavailable".into(),
        ))
    }

    async fn retain(
        &self,
        sandbox_id: &SandboxId,
        owner: &SubjectId,
        services: &[ServiceId],
        credential: &ProvidedCredential,
    ) -> Result<(), CoreError>;
}

impl super::ProviderBinding {
    pub fn require_credential_admission(
        &self,
        credential: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        self.validate().map_err(CoreError::InvalidState)?;
        if self.state != super::BindingState::Provisioning
            || self.work != super::BindingWork::AwaitingCredential
            || credential.sha256() != self.key_sha256
            || self.profile.environment.is_none()
        {
            return Err(CoreError::InvalidState(
                "binding does not admit credentials".into(),
            ));
        }
        Ok(())
    }

    pub fn require_credential_retirement(&self, id: &SandboxId) -> Result<(), CoreError> {
        self.validate().map_err(CoreError::InvalidState)?;
        let allowed = (self.state == super::BindingState::Retired
            && self.work == super::BindingWork::RetireCurrentSandbox
            && *id == self.sandbox_id)
            || (self.work == super::BindingWork::RetirePreviousSandbox
                && self
                    .previous
                    .as_ref()
                    .is_some_and(|old| old.retire_sandbox && old.sandbox_id == *id));
        if !allowed {
            return Err(CoreError::InvalidState(
                "binding does not retire credentials".into(),
            ));
        }
        Ok(())
    }
}
