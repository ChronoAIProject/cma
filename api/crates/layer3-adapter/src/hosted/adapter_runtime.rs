use std::collections::BTreeSet;

use super::{HostedAdapter, RuntimeState};
use crate::hosted::deployment::{provider_request_identity, ProviderIdentity};
use crate::{AdapterError, RuntimeAdoptionRequest, RuntimeHandle, RuntimeObservation, SandboxSpec};

impl HostedAdapter {
    pub(super) async fn provision_exact(
        &self,
        spec: &SandboxSpec,
        generation: u64,
    ) -> crate::Result<RuntimeHandle> {
        let request = self.deployment.render_runtime(spec, generation)?;
        let identity = provider_request_identity(&request)?;
        let handle = self.backend.provision(request).await?;
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
            .render_runtime(&adoption.spec, adoption.generation)?;
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
