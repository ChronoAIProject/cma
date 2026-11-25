//! Authorize immutable public source preparation before a new allocation.

use super::*;
use layer2_core::profiles::authoring::{RepositoryRequest, RepositoryResolution};

#[cfg(test)]
#[async_trait]
pub(in crate::agent_lifecycle) trait PublicRepositoryReadiness:
    Send + Sync
{
    async fn resolve(
        &self,
        request: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError>;
}

impl InlineAgentLifecycle {
    pub(crate) fn with_public_repository_images(
        self: Arc<Self>,
        images: std::collections::BTreeSet<String>,
        writer_enabled: bool,
    ) -> Arc<Self> {
        self.public_repository_writer
            .set(writer_enabled)
            .expect("public repository writer is configured once before use");
        self.public_repository_images
            .set(images)
            .expect("public repository qualification is configured once before use");
        self
    }

    #[cfg(test)]
    pub(in crate::agent_lifecycle) fn with_public_repository_probe(
        self: Arc<Self>,
        probe: Arc<dyn PublicRepositoryReadiness>,
    ) -> Arc<Self> {
        assert!(self.public_repository_probe.set(probe).is_ok());
        self
    }

    pub(super) async fn try_public_repository(
        &self,
        input: &mut ProvisionInput,
    ) -> Result<bool, AgentLifecycleProvisioningError> {
        if input.first_message.is_none()
            || input.existing_sandbox_id.is_some()
            || !self.public_repository_allocation_enabled(input)
        {
            return Ok(false);
        }
        let repository = input
            .request
            .repository
            .as_ref()
            .ok_or(AgentLifecycleProvisioningError::Unavailable)?;
        let request = RepositoryRequest {
            url: repository.url.clone(),
            reference: repository.reference.clone(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            self.resolve_public_repository(&request),
        )
        .await;
        let Ok(Ok(resolved)) = result else {
            return Ok(false);
        };
        let pin = PublicRepositoryPin {
            url: resolved.url,
            commit: resolved.commit_sha,
        };
        if layer3_adapter::public_repository_archive_url(&pin.url, &pin.commit).is_none() {
            return Ok(false);
        }
        self.operations
            .pin_public_repository(&input.lease, &pin)
            .await
            .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
        Self::apply_public_repository_pin(input, &pin)?;
        Ok(true)
    }

    pub(super) async fn preflight_public_repository(
        &self,
        caller: &Caller,
        input: &mut ProvisionInput,
        pin: &PublicRepositoryPin,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        if input.first_message.is_none() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if input.existing_sandbox_id.is_none() {
            // Allocation and the operation marker are separate commits. Recover
            // the exact owned origin before applying new-allocation policy.
            if let Some(id) = self
                .public_repository_allocation(caller, &input.lease.id)
                .await?
            {
                self.operations
                    .mark_sandbox(&input.lease, id.as_str())
                    .await
                    .map_err(|_| AgentLifecycleProvisioningError::Unavailable)?;
                input.existing_sandbox_id = Some(id.to_string());
            } else {
                if !self.public_repository_allocation_enabled(input) {
                    return self.wait_for_public_repository_access(caller, input).await;
                }
                let request = RepositoryRequest {
                    url: pin.url.clone(),
                    reference: Some(pin.commit.clone()),
                };
                let check = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.resolve_public_repository(&request),
                )
                .await;
                if !matches!(check, Ok(Ok(resolved)) if resolved.url == pin.url && resolved.commit_sha == pin.commit)
                {
                    return self.wait_for_public_repository_access(caller, input).await;
                }
            }
        }
        Self::apply_public_repository_pin(input, pin)
    }

    fn apply_public_repository_pin(
        input: &mut ProvisionInput,
        pin: &PublicRepositoryPin,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        if layer3_adapter::public_repository_archive_url(&pin.url, &pin.commit).is_none() {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        let repository = input
            .request
            .repository
            .as_mut()
            .ok_or(AgentLifecycleProvisioningError::Unavailable)?;
        repository.url = pin.url.clone();
        repository.reference = Some(pin.commit.clone());
        input.behavior = LifecycleBehavior::PublicRepository;
        Ok(())
    }

    fn public_repository_allocation_enabled(&self, input: &ProvisionInput) -> bool {
        self.public_repository_writer.get() == Some(&true)
            && self.managed_repository_preflight.get() == Some(&true)
            && input.runtime_image.as_ref().is_some_and(|image| {
                self.public_repository_images
                    .get()
                    .is_some_and(|images| images.contains(image))
            })
    }

    async fn public_repository_allocation(
        &self,
        caller: &Caller,
        operation_id: &str,
    ) -> Result<Option<SandboxId>, AgentLifecycleProvisioningError> {
        let origin = layer2_core::sandbox::SandboxOrigin::Lifecycle {
            operation_id: operation_id.into(),
        };
        let mut page = layer2_core::sandbox::store::PageRequest::default();
        let mut found = None;
        loop {
            let result = self
                .sandbox
                .list_own(caller, &page)
                .await
                .map_err(sandbox_error)?;
            for sandbox in result
                .items
                .into_iter()
                .filter(|sandbox| sandbox.origin == origin)
            {
                if found.replace(sandbox.id).is_some() {
                    return Err(AgentLifecycleProvisioningError::Conflict(
                        "the operation has multiple Sandbox allocations".into(),
                    ));
                }
            }
            match result.next {
                Some(cursor) => page.cursor = Some(cursor),
                None => return Ok(found),
            }
        }
    }

    async fn resolve_public_repository(
        &self,
        request: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        #[cfg(test)]
        if let Some(probe) = self.public_repository_probe.get() {
            return probe.resolve(request).await;
        }
        crate::profile_authoring::resolve_public_snapshot(request).await
    }

    async fn wait_for_public_repository_access(
        &self,
        caller: &Caller,
        input: &ProvisionInput,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        self.wait_for_managed_repository_access(
            caller,
            input,
            RepositoryAccessRefusal::VerificationUnavailable,
            None,
        )
        .await
    }
}
