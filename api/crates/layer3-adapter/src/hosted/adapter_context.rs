//! Shared fresh authority is local to this adoption/status call, never a drain.
use super::{runtime_lifecycle::validate_inventory, HostedAdapter};
use crate::{AdapterError, RuntimeAdoptionRequest, RuntimeInspection};

impl HostedAdapter {
    pub(super) async fn inspect_exact(
        &self,
        adoption: &RuntimeAdoptionRequest,
    ) -> crate::Result<Option<RuntimeInspection>> {
        if let Some(home) = adoption
            .spec
            .home_volume
            .as_deref()
            .filter(|home| home.request.policy.is_native_vct())
        {
            home.validate_for(&adoption.spec.sandbox_id)?;
            self.verify_adoption_home_route(adoption)?;
            let homes = self.home_volumes.as_ref().ok_or_else(|| {
                AdapterError::Unsupported("durable home storage is not configured".into())
            })?;
            if home
                .request
                .scope
                .as_ref()
                .is_none_or(|scope| !homes.accepts_scope(scope))
            {
                return Err(AdapterError::Rejected(
                    "native home belongs to another authority".into(),
                ));
            }
            let request = self
                .deployment
                .render_adoption(adoption, adoption.generation)?;
            let expected = crate::hosted::deployment::provider_request_identity(&request)?;
            if let Some(inspection) = self
                .backend
                .inspect_native_runtime(&adoption.handle, &request, home)
                .await?
            {
                let observed = &inspection.observation;
                validate_inventory(std::slice::from_ref(observed))?;
                if observed.handle != adoption.handle
                    || observed.sandbox_id != expected.sandbox_id
                    || observed.generation != expected.generation
                    || observed.image_sha256 != expected.image_sha256
                    || observed.spec_fingerprint != expected.spec_fingerprint
                    || observed.runtime_contract != expected.runtime_contract
                    || !observed.lifecycle.is_running()
                {
                    return Err(AdapterError::Rejected(
                        "provider did not prove the exact running runtime identity".into(),
                    ));
                }
                let status = inspection.status.as_ref().ok_or_else(|| {
                    AdapterError::Rejected("native runtime status missing".into())
                })?;
                status.validate_for(&crate::RuntimeStatusRequest {
                    sandbox_id: expected.sandbox_id.clone(),
                    generation: expected.generation,
                    home_identity: home.identity()?,
                })?;
                // The post-status observation boundary is deliberately fresh.
                self.verify_home(home).await?;
                self.attach_state(adoption.handle.clone(), &adoption.spec, expected)
                    .await?;
                return Ok(Some(inspection));
            }
        }
        let Some(observation) = self.adopt_exact(adoption).await? else {
            return Ok(None);
        };
        let status = if adoption.spec.home_volume.is_some() {
            Some(self.inspect_runtime_status(&adoption.handle).await?)
        } else {
            None
        };
        Ok(Some(RuntimeInspection {
            observation,
            status,
        }))
    }
}
