use crate::file_read_diagnostics::{measure, Stage};
use std::collections::BTreeSet;

use super::runtime_identity::{exact_identity_predicate, identity_rejection};
use super::{HostedAdapter, RuntimeState};
use crate::hosted::deployment::{provider_request_identity, ProviderIdentity};
use crate::{AdapterError, RuntimeAdoptionRequest, RuntimeHandle, RuntimeObservation, SandboxSpec};

#[path = "adapter_runtime_transition.rs"]
mod transition;

impl HostedAdapter {
    pub(super) async fn provision_receipted_exact(
        &self,
        request: &crate::RuntimeProvisionRequest,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeProvisionReceipt> {
        let mut rendered = self.deployment.render_request(request)?;
        rendered.funded_until = request.funded_until;
        self.verify_spec_home(&request.spec).await?;
        if let Some(home) = &request.spec.home_volume {
            self.backend
                .verify_home_route(&rendered, &home.request)
                .await?;
        }
        self.backend.validate_funding(&rendered, None).await?;
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
        measure(Stage::HomeBeforeStatus, self.verify_home(home)).await?;
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
        measure(Stage::HomeAfterStatus, self.verify_home(home)).await?;
        Ok(status)
    }

    pub(super) fn retained_request(
        &self,
        request: &RuntimeAdoptionRequest,
        generation: u64,
    ) -> crate::Result<crate::provider::ProviderSandboxRequest> {
        if invalid_handle(&request.handle) || request.spec.home_volume.is_none() {
            return Err(AdapterError::Rejected(
                "native suspension requires an exact runtime and persistent home".into(),
            ));
        }
        self.deployment.render_adoption(request, generation)
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

    pub(super) async fn inspect_retained_exact(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<crate::RetainedStartupPolicyPredecessor> {
        let rendered = self.retained_request(request, request.generation)?;
        let home = request.spec.home_volume.as_deref().ok_or_else(|| {
            crate::AdapterError::Rejected("retained policy inspection requires a home".into())
        })?;
        self.verify_spec_home(&request.spec).await?;
        self.verify_adoption_home_route(request)?;
        crate::hosted::retained_startup_policy::require(self.backend.as_ref())?
            .inspect_retained_startup_policy(&request.handle, &rendered, home)
            .await
    }

    pub(super) async fn provision_exact(
        &self,
        spec: &SandboxSpec,
        generation: u64,
        funded_until: Option<u64>,
    ) -> crate::Result<RuntimeHandle> {
        let mut request = self.deployment.render_runtime(spec, generation)?;
        request.funded_until = funded_until;
        self.verify_spec_home(spec).await?;
        if let Some(home) = &spec.home_volume {
            self.backend
                .verify_home_route(&request, &home.request)
                .await?;
        }
        self.backend.validate_funding(&request, None).await?;
        let identity = provider_request_identity(&request)?;
        let handle = match spec.home_volume.as_deref() {
            Some(home) => self.backend.provision_with_home(request, home).await?,
            None => self.backend.provision(request).await?,
        };
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
        let request = self
            .deployment
            .render_adoption(adoption, adoption.generation)?;
        self.verify_spec_home(&adoption.spec).await?;
        self.verify_adoption_home_route(adoption)?;
        let expected = provider_request_identity(&request)?;
        let observed = match adoption.spec.home_volume.as_deref() {
            Some(home) => {
                self.backend
                    .adopt_with_home(&adoption.handle, &request, home)
                    .await?
            }
            None => self.backend.adopt(&adoption.handle, &request).await?,
        };
        let Some(observation) = observed else {
            return Ok(None);
        };
        if validate_inventory(std::slice::from_ref(&observation)).is_err() {
            return Err(identity_rejection("inventory"));
        }
        if let Some(predicate) = exact_identity_predicate(&observation, &adoption.handle, &expected)
        {
            return Err(identity_rejection(predicate));
        }
        self.attach_adopted_state(adoption.handle.clone(), &adoption.spec, expected)
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

    pub(super) async fn attach_state(
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

    /// Replace only an exact older generation after the provider has proved the
    /// newer identity and rebound its transport. A same-generation or foreign
    /// identity remains a hard conflict; the cache is never an authority source.
    pub(super) async fn attach_adopted_state(
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
        match states.get(&handle) {
            None => {
                states.insert(handle.clone(), candidate);
                Ok(handle)
            }
            Some(current) if current == &candidate => Ok(handle),
            Some(current) if supersedes(current, &candidate) => {
                states.insert(handle.clone(), candidate);
                Ok(handle)
            }
            Some(_) => Err(AdapterError::Rejected(format!(
                "provider reused live runtime handle {}",
                handle.0
            ))),
        }
    }

    pub(super) fn verify_adoption_home_route(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> crate::Result<()> {
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

    pub(super) async fn verify_home(
        &self,
        home: &crate::HomeVolumeAttachment,
    ) -> crate::Result<()> {
        self.home_volumes
            .as_ref()
            .ok_or_else(|| {
                AdapterError::Unsupported("durable home storage is not configured".into())
            })?
            .verify(home)
            .await
    }
}

fn supersedes(current: &RuntimeState, candidate: &RuntimeState) -> bool {
    current.provision.sandbox_id == candidate.provision.sandbox_id
        && current.provision.generation < candidate.provision.generation
        && current.own_credential == candidate.own_credential
        && current.home_directory == candidate.home_directory
        && current.home_volume == candidate.home_volume
}

pub(super) fn validate_inventory(inventory: &[RuntimeObservation]) -> crate::Result<()> {
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

#[cfg(test)]
#[path = "adapter_runtime_tests.rs"]
mod tests;

impl HostedAdapter {
    pub(super) async fn deadline_exact(
        &self,
        request: &RuntimeAdoptionRequest,
        until: u64,
    ) -> crate::Result<()> {
        let rendered = self
            .deployment
            .render_adoption(request, request.generation)?;
        self.backend
            .set_runtime_deadline(&request.handle, &rendered, until)
            .await
    }
    pub(super) async fn resources_exact(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::ResourceReading> {
        self.known_state(handle).await?;
        match self
            .exchange(handle, crate::wire::RequestFrame::Resources)
            .await?
        {
            crate::wire::ResponseFrame::Resources(reading) => Ok(reading),
            _ => unreachable!("exchange verifies response operation"),
        }
    }
    pub(super) async fn teardown_exact(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        self.known_state(handle).await?;
        self.backend.teardown(handle).await?;
        self.states.lock().await.remove(handle);
        Ok(())
    }
}
