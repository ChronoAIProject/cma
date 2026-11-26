use async_trait::async_trait;
use layer1_public::rest::{AgentLifecycleImageReference, AgentLifecycleProvisioningError};

#[async_trait]
pub(crate) trait ImageResolver: Send + Sync {
    fn retirement_enabled(&self) -> bool {
        false
    }

    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError>;

    async fn resolve_retained(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
        _pinned_reference: &str,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        self.resolve(delegated_credential, image).await
    }
}

/// Refuse new custom image selections while preserving persisted runtime pins.
pub(crate) struct RetiredImageResolver;

#[async_trait]
impl ImageResolver for RetiredImageResolver {
    fn retirement_enabled(&self) -> bool {
        true
    }

    async fn resolve(
        &self,
        _delegated_credential: Option<&str>,
        _image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported)
    }
}

pub(crate) fn valid_immutable_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 2_048
        && !name.contains('@')
        && !name.contains("://")
        && !name.starts_with(['.', '-', '_', '/', ':'])
        && !name.ends_with(['.', '-', '_', '/', ':'])
        && !name.contains("//")
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
