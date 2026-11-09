//! A captured reply cannot outlive its native authority or process assignment.
use super::*;

impl AgentSandboxBackend {
    pub(super) async fn exchange_bound_status(
        &self,
        handle: &RuntimeHandle,
        request: RuntimeStatusRequest,
    ) -> Result<ResponseFrame> {
        let assigned = self.assignments.lock().unwrap().get(handle).cloned();
        if let Some(assigned) = &assigned {
            let identity = crate::hosted::provider_request_identity(&assigned.request)?;
            if request.sandbox_id != identity.sandbox_id
                || request.generation != identity.generation
                || assigned.request.metadata.get("cma/home-identity")
                    != Some(&request.home_identity)
            {
                return Err(AdapterError::Rejected(
                    "status request differs from its acknowledged assignment".into(),
                ));
            }
        } else {
            // Eviction or a missing incarnation requires adoption/bootstrap to
            // supply a new acknowledgement; it must not bypass native proof.
            let object = self
                .client
                .get(handle)
                .await?
                .ok_or_else(|| AdapterError::Unavailable("status runtime is absent".into()))?;
            if object
                .annotations()
                .contains_key(crate::agent_sandbox::NATIVE_HOME_ANNOTATION)
            {
                return Err(AdapterError::Unavailable(
                    "native status requires a fresh acknowledged assignment".into(),
                ));
            }
        }
        let response = self
            .transport
            .exchange(handle, RequestFrame::RuntimeStatus(request))
            .await?;
        if let Some(assigned) = assigned {
            timed(Stage::NativeIdentity, async {
                let object = self
                    .client
                    .validate_request(handle, &assigned.request)
                    .await?
                    .ok_or_else(|| {
                        AdapterError::Unavailable("status runtime disappeared".into())
                    })?;
                self.client.verify_explicit_runtime_policy(&object).await?;
                let pod = self.client.pod_binding(handle).await?;
                if pod != assigned.pod
                    || self.assignments.lock().unwrap().get(handle) != Some(&assigned)
                {
                    return Err(AdapterError::Rejected(
                        "native status assignment changed during exchange".into(),
                    ));
                }
                Ok(())
            })
            .await?;
        }
        Ok(response)
    }
}
