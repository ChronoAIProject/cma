//! New work renders the release it was admitted against.
use super::HostedDeployment;
use crate::provider::ProviderSandboxRequest;

impl HostedDeployment {
    /// Render a provisioning request. Its recorded image wins; new work without
    /// one uses the release it was admitted against, then the bootstrap image.
    pub fn render_request(
        &self,
        request: &crate::RuntimeProvisionRequest,
    ) -> crate::Result<ProviderSandboxRequest> {
        match (&request.spec.runtime_image, &request.released) {
            (None, Some(released)) => {
                let mut spec = request.spec.clone();
                spec.runtime_image = Some(released.agent.clone());
                self.render_runtime(&spec, request.generation)
            }
            _ => self.render_runtime(&request.spec, request.generation),
        }
    }
}
