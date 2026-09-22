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
    async fn retain(
        &self,
        sandbox_id: &SandboxId,
        owner: &SubjectId,
        services: &[ServiceId],
        credential: &ProvidedCredential,
    ) -> Result<(), CoreError>;
}
