//! One-operation native authority and process inspection.
use super::*;

pub(super) fn observation(object: &DynamicObject) -> Result<RuntimeObservation> {
    let identity = crate::hosted::deployment::provider_metadata_identity(object.annotations())?;
    let ready = object
        .data
        .pointer("/status/conditions")
        .and_then(|v| v.as_array())
        .is_some_and(|cs| {
            cs.iter().any(|c| {
                c["type"] == "Ready"
                    && c["status"] == "True"
                    && c["observedGeneration"].as_i64() == object.metadata.generation
            })
        });
    let state = if object.metadata.deletion_timestamp.is_some() {
        "terminating"
    } else if object
        .data
        .pointer("/spec/operatingMode")
        .and_then(|value| value.as_str())
        == Some("Suspended")
    {
        "suspended"
    } else if ready {
        "running"
    } else {
        "pending"
    };
    Ok(RuntimeObservation {
        sandbox_id: identity.sandbox_id,
        generation: identity.generation,
        handle: crate::agent_sandbox::handle(object)?,
        image_sha256: identity.image_sha256,
        spec_fingerprint: identity.spec_fingerprint,
        runtime_contract: identity.runtime_contract,
        lifecycle: RuntimeLifecycle {
            state: state.into(),
            reason: None,
        },
    })
}

impl AgentSandboxBackend {
    pub(super) async fn inspect_native_context(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeInspection>> {
        if !home.request.policy.is_native_vct() {
            return Ok(None);
        }
        let (proof, policy) = tokio::join!(
            self.client.native_runtime_proof(handle, request, home),
            self.client.verify_native_runtime_policy(handle, home),
        );
        // Consume graph errors and the bootstrap fallback before policy errors,
        // preserving the original authority/error precedence of this operation.
        let proof = proof?;
        if !proof.matches(request, home)?
            || !self.acknowledged_assignment(handle, proof.pod(), request)?
        {
            // An initial/replaced process must run the original bootstrap protocol.
            return Ok(None);
        }
        policy?;
        if !self.acknowledged_assignment(handle, proof.pod(), request)? {
            return Err(AdapterError::Unavailable(
                "runtime assignment retired during inspection".into(),
            ));
        }
        self.bind_transport(handle, request).await?;
        let observed = observation(proof.member())?;
        let identity = crate::hosted::deployment::provider_request_identity(request)?;
        let status_request = RuntimeStatusRequest {
            sandbox_id: identity.sandbox_id,
            generation: identity.generation,
            home_identity: home.identity()?,
        };
        // Transport still obtains fresh Pod authority immediately before dispatch.
        let response = self
            .transport
            .exchange(handle, RequestFrame::RuntimeStatus(status_request.clone()))
            .await?;
        let ResponseFrame::RuntimeStatus(status) = response else {
            return Err(AdapterError::unexpected_reply("runtime status"));
        };
        status.validate_for(&status_request)?;
        Ok(Some(RuntimeInspection {
            observation: observed,
            status: Some(status),
        }))
    }
}
