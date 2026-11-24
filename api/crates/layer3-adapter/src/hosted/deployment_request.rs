//! New work renders the release it was admitted against.
use super::HostedDeployment;
use crate::provider::ProviderSandboxRequest;
use crate::{RuntimeProvisionRequest, SandboxSpec};
use std::borrow::Cow;

impl RuntimeProvisionRequest {
    pub(in crate::hosted) fn released_spec(&self) -> Cow<'_, SandboxSpec> {
        match (&self.spec.runtime_image, &self.released) {
            (None, Some(released)) => {
                let mut spec = self.spec.clone();
                spec.runtime_image = Some(released.agent.clone());
                Cow::Owned(spec)
            }
            _ => Cow::Borrowed(&self.spec),
        }
    }
}

impl HostedDeployment {
    /// A recorded image wins over the release captured for new work.
    pub fn render_request(
        &self,
        request: &RuntimeProvisionRequest,
    ) -> crate::Result<ProviderSandboxRequest> {
        self.render_runtime(&request.released_spec(), request.generation)
    }
}
