use std::collections::BTreeSet;

use super::{HostedAdapter, RuntimeState};
use crate::hosted::deployment::{provider_request_identity, ProviderIdentity};
use crate::{AdapterError, RuntimeAdoptionRequest, RuntimeHandle, RuntimeObservation, SandboxSpec};

impl HostedAdapter {
    pub(super) async fn provision_receipted_exact(
        &self,
        request: &crate::RuntimeProvisionRequest,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeProvisionReceipt> {
        let rendered = self
            .deployment
            .render_runtime(&request.spec, request.generation)?;
        self.verify_spec_home(&request.spec).await?;
        if let Some(home) = &request.spec.home_volume {
            self.backend
                .verify_home_route(&rendered, &home.request)
                .await?;
        }
        let identity = provider_request_identity(&rendered)?;
        let request_sha256 = crate::provider_request_digest(request)?;
        let receipt = self
            .backend
            .provision_receipted(rendered, operation_id, &request_sha256)
            .await?;
        receipt.validate()?;
        if receipt.operation_id() != operation_id || receipt.request_sha256() != request_sha256 {
            return Err(AdapterError::Rejected(
                "provider returned another original dispatch receipt".into(),
            ));
        }
        self.verify_spec_home(&request.spec).await?;
        self.attach_state(receipt.handle().clone(), &request.spec, identity)
            .await?;
        Ok(receipt)
    }
    pub(super) async fn inspect_runtime_status(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::RuntimeStatus> {
        let state = self.known_state(handle).await?;
        let home = state
            .home_volume
            .as_ref()
            .ok_or_else(|| AdapterError::Unsupported("runtime has no persistent home".into()))?;
        self.verify_home(home).await?;
        let request = crate::RuntimeStatusRequest {
            sandbox_id: state.provision.sandbox_id,
            generation: state.provision.generation,
            home_identity: home.identity()?,
        };
        let response = self
            .exchange(
                handle,
                crate::wire::RequestFrame::RuntimeStatus(request.clone()),
            )
            .await?;
        let crate::wire::ResponseFrame::RuntimeStatus(status) = response else {
            return Err(AdapterError::unexpected_reply("runtime status"));
        };
        status.validate_for(&request)?;
        self.verify_home(home).await?;
        Ok(status)
    }

    fn retained_request(
        &self,
        request: &RuntimeAdoptionRequest,
        generation: u64,
    ) -> crate::Result<crate::provider::ProviderSandboxRequest> {
        if invalid_handle(&request.handle) || request.spec.home_volume.is_none() {
            return Err(AdapterError::Rejected(
                "native suspension requires an exact runtime and persistent home".into(),
            ));
        }
        let mut deployment = self.deployment.clone();
        if let Some(image) = &request.expected_image {
            deployment.image.uri = image.clone();
        }
        deployment.render_runtime(&request.spec, generation)
    }

    pub(super) async fn suspend_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        observe_only: bool,
    ) -> crate::Result<()> {
        let rendered = self.retained_request(request, request.generation)?;
        self.verify_spec_home(&request.spec).await?;
        self.verify_adoption_home_route(request)?;
        if observe_only {
            self.backend
                .verify_suspended(&request.handle, &rendered)
                .await?;
        } else {
            self.backend.suspend(&request.handle, &rendered).await?;
            self.states.lock().await.remove(&request.handle);
        }
        self.verify_spec_home(&request.spec).await
    }

    pub(super) async fn resume_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        successor: &crate::RuntimeProvisionRequest,
    ) -> crate::Result<RuntimeHandle> {
        let mut allowed = request.spec.clone();
        allowed.reaches.clone_from(&successor.spec.reaches);
        if successor.generation <= request.generation || successor.spec != allowed {
            return Err(AdapterError::Rejected(
                "resume must advance generation and may change only service scope".into(),
            ));
        }
        let previous = self.retained_request(request, request.generation)?;
        // Preserve the persisted image even when the deployment default changed.
        let mut resumed = request.clone();
        resumed.spec = successor.spec.clone();
        resumed.generation = successor.generation;
        let next = self.retained_request(&resumed, successor.generation)?;
        let previous_identity = provider_request_identity(&previous)?;
        let next_identity = provider_request_identity(&next)?;
        self.verify_spec_home(&request.spec).await?;
        self.verify_adoption_home_route(request)?;
        {
            let mut states = self.states.lock().await;
            if let Some(state) = states.get(&request.handle) {
                if state.provision != previous_identity && state.provision != next_identity {
                    return Err(AdapterError::Rejected(
                        "resume conflicts with another runtime generation".into(),
                    ));
                }
            }
            states.remove(&request.handle);
        }
        self.backend
            .resume(&request.handle, &previous, &next)
            .await?;
        self.verify_spec_home(&request.spec).await?;
        self.attach_state(request.handle.clone(), &successor.spec, next_identity)
            .await
    }

    pub(super) async fn provision_exact(
        &self,
        spec: &SandboxSpec,
        generation: u64,
    ) -> crate::Result<RuntimeHandle> {
        let request = self.deployment.render_runtime(spec, generation)?;
        self.verify_spec_home(spec).await?;
        if let Some(home) = &spec.home_volume {
            self.backend
                .verify_home_route(&request, &home.request)
                .await?;
        }
        let identity = provider_request_identity(&request)?;
        let handle = self.backend.provision(request).await?;
        self.verify_spec_home(spec).await?;
        self.attach_state(handle, spec, identity).await
    }

    pub(super) async fn inventory_exact(&self) -> crate::Result<Vec<RuntimeObservation>> {
        let mut inventory = self.backend.inventory().await?;
        validate_inventory(&inventory)?;
        inventory.sort_by(|left, right| {
            (&left.sandbox_id, left.generation, &left.handle).cmp(&(
                &right.sandbox_id,
                right.generation,
                &right.handle,
            ))
        });
        Ok(inventory)
    }

    pub(super) async fn adopt_exact(
        &self,
        adoption: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<RuntimeObservation>> {
        if invalid_handle(&adoption.handle) {
            return Err(AdapterError::Rejected(
                "runtime adoption handle is invalid".into(),
            ));
        }
        let mut deployment = self.deployment.clone();
        if let Some(image) = &adoption.expected_image {
            deployment.image.uri = image.clone();
        }
        let request = deployment.render_runtime(&adoption.spec, adoption.generation)?;
        self.verify_spec_home(&adoption.spec).await?;
        self.verify_adoption_home_route(adoption)?;
        let expected = provider_request_identity(&request)?;
        let Some(observation) = self.backend.adopt(&adoption.handle, &request).await? else {
            return Ok(None);
        };
        if validate_inventory(std::slice::from_ref(&observation)).is_err()
            || observation.handle != adoption.handle
            || observation.sandbox_id != expected.sandbox_id
            || observation.generation != expected.generation
            || observation.image_sha256 != expected.image_sha256
            || observation.spec_fingerprint != expected.spec_fingerprint
            || observation.runtime_contract != expected.runtime_contract
            || !observation.lifecycle.is_running()
        {
            return Err(AdapterError::Rejected(
                "provider did not prove the exact running runtime identity".into(),
            ));
        }
        self.attach_state(adoption.handle.clone(), &adoption.spec, expected)
            .await?;
        self.verify_spec_home(&adoption.spec).await?;
        Ok(Some(observation))
    }

    pub(super) async fn reclaim_exact(
        &self,
        observation: &RuntimeObservation,
    ) -> crate::Result<()> {
        validate_inventory(std::slice::from_ref(observation))?;
        self.backend.reclaim(observation).await?;
        self.states.lock().await.remove(&observation.handle);
        Ok(())
    }

    async fn attach_state(
        &self,
        handle: RuntimeHandle,
        spec: &SandboxSpec,
        provision: ProviderIdentity,
    ) -> crate::Result<RuntimeHandle> {
        if invalid_handle(&handle) {
            return Err(AdapterError::Rejected(
                "provider returned an invalid runtime handle".into(),
            ));
        }
        let candidate = RuntimeState {
            own_credential: spec.own_credential,
            home_directory: spec.home_directory.clone(),
            provision,
            home_volume: spec.home_volume.clone(),
        };
        let mut states = self.states.lock().await;
        if let Some(state) = states.get(&handle) {
            if state == &candidate {
                return Ok(handle);
            }
            return Err(AdapterError::Rejected(format!(
                "provider reused live runtime handle {}",
                handle.0
            )));
        }
        states.insert(handle.clone(), candidate);
        Ok(handle)
    }

    fn verify_adoption_home_route(&self, request: &RuntimeAdoptionRequest) -> crate::Result<()> {
        if let Some(home) = &request.spec.home_volume {
            self.backend
                .verify_retained_home_route(&request.handle, &home.request)?;
        }
        Ok(())
    }

    async fn verify_spec_home(&self, spec: &SandboxSpec) -> crate::Result<()> {
        if let Some(home) = &spec.home_volume {
            home.validate_for(&spec.sandbox_id)?;
            self.verify_home(home).await?;
        }
        Ok(())
    }

    pub(super) async fn verify_bound_home(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        let state = self.known_state(handle).await?;
        if let Some(home) = &state.home_volume {
            self.verify_home(home).await?;
        }
        Ok(())
    }

    async fn verify_home(&self, home: &crate::HomeVolumeAttachment) -> crate::Result<()> {
        self.home_volumes
            .as_ref()
            .ok_or_else(|| {
                AdapterError::Unsupported("durable home storage is not configured".into())
            })?
            .verify(home)
            .await
    }
}

fn validate_inventory(inventory: &[RuntimeObservation]) -> crate::Result<()> {
    let mut handles = BTreeSet::new();
    let mut generations = BTreeSet::new();
    for observation in inventory {
        if invalid_handle(&observation.handle)
            || observation.sandbox_id.trim().is_empty()
            || observation.sandbox_id.trim() != observation.sandbox_id
            || observation.sandbox_id.len() > 256
            || observation.sandbox_id.chars().any(char::is_control)
            || observation.generation == 0
            || !sha256_fingerprint(&observation.image_sha256)
            || observation.spec_fingerprint.len() != 43
            || !observation
                .spec_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || observation.runtime_contract.trim().is_empty()
            || observation.runtime_contract.trim() != observation.runtime_contract
            || observation.runtime_contract.len() > 64
            || observation.runtime_contract.chars().any(char::is_control)
            || observation.lifecycle.state.trim().is_empty()
            || observation.lifecycle.state.len() > 128
            || observation.lifecycle.state.chars().any(char::is_control)
            || observation.lifecycle.reason.as_ref().is_some_and(|reason| {
                reason.chars().count() > 1_024
                    || reason.chars().any(|character| {
                        character.is_control() && !matches!(character, '\n' | '\t')
                    })
            })
            || !handles.insert(observation.handle.clone())
            || !generations.insert((observation.sandbox_id.clone(), observation.generation))
        {
            return Err(AdapterError::Rejected(
                "hosted runtime inventory contains an invalid or ambiguous identity".into(),
            ));
        }
    }
    Ok(())
}

fn invalid_handle(handle: &RuntimeHandle) -> bool {
    handle.0.trim().is_empty()
        || handle.0.trim() != handle.0
        || handle.0.len() > 512
        || handle.0.chars().any(char::is_control)
}

fn sha256_fingerprint(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
