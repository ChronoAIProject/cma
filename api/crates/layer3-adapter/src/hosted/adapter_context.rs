//! Shared fresh authority is local to this adoption/status call, never a drain.
use super::{runtime_lifecycle::validate_inventory, HostedAdapter};
use crate::hosted::inspection_timing;
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
            if let Some(inspection) = inspection_timing::measure(
                &adoption.spec.sandbox_id,
                "native_observation",
                self.backend
                    .inspect_native_runtime(&adoption.handle, &request, home),
            )
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
                inspection_timing::measure(
                    &adoption.spec.sandbox_id,
                    "post_status_home",
                    self.verify_home(home),
                )
                .await?;
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

impl HostedAdapter {
    pub(super) async fn bind_context_refresh(
        &self,
        handle: &crate::RuntimeHandle,
        request: &crate::AgentOpenRequest,
        settings: &super::super::HostedAgentOpenSettings,
    ) -> crate::Result<Option<super::super::HostedContextRefresh>> {
        let state = self.known_state(handle).await?;
        let Some(home) = state
            .home_volume
            .as_ref()
            .filter(|home| home.request.policy.mounts_full_home())
        else {
            return Ok(None);
        };
        let tools = settings
            .channel_tool
            .as_ref()
            .map(|b| b.acp_server())
            .into_iter()
            .chain(settings.provider_tool.as_ref().map(|b| b.acp_server()))
            .chain(settings.access_tool.as_ref().map(|b| b.acp_server()))
            .collect();
        super::super::HostedContextRefresh::new(
            self.backend.clone(),
            handle.clone(),
            crate::RuntimeStatusRequest {
                sandbox_id: state.provision.sandbox_id,
                generation: state.provision.generation,
                home_identity: home.identity()?,
            },
            &request.options.home_directory,
            request.options.working_directory.clone(),
            tools,
        )
        .map(|context| {
            let context = match (&self.access_tools, &settings.access_tool) {
                (Some(source), Some(binding)) => {
                    context.with_access(source.clone(), binding.clone())
                }
                _ => context,
            };
            Some(match (&self.provider_tools, &settings.provider_tool) {
                (Some(source), Some(binding)) => {
                    context.with_provider(source.clone(), binding.clone())
                }
                _ => context,
            })
        })
    }
}
