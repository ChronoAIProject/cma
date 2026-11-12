use async_trait::async_trait;
use layer1_public::rest::{AgentLifecycleImageReference, AgentLifecycleProvisioningError};

#[async_trait]
pub(crate) trait ImageResolver: Send + Sync {
    async fn resolve(
        &self,
        delegated_credential: Option<&str>,
        image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError>;
}

/// Rejects custom image selection before admission without contacting an external service.
pub(crate) struct UnsupportedImageResolver;

#[async_trait]
impl ImageResolver for UnsupportedImageResolver {
    async fn resolve(
        &self,
        _delegated_credential: Option<&str>,
        _image: &AgentLifecycleImageReference,
    ) -> Result<String, AgentLifecycleProvisioningError> {
        Err(AgentLifecycleProvisioningError::ImageSelectionUnsupported)
    }
}
