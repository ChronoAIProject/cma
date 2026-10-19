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
    pub(super) async fn bind_resumed(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        let mut timing = crate::agent_sandbox::resume_timing::ResumeTiming::new();
        let pod = self
            .client
            .wait_ready(handle, Duration::from_secs(180))
            .await?;
        timing.completed("wait_ready");
        self.assign_and_bind(handle, &pod, request).await
    }

    pub(super) async fn inspect_native_context(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeInspection>> {
        if !home.request.policy.is_native_vct() {
            return Ok(None);
        }
        let inspection = NativeStatusInspection {
            backend: self,
            handle,
            request,
            home,
        };
        if let Some(inspected) = self.transport.inspect_native_status(&inspection).await? {
            return Ok(Some(inspected));
        }
        let Some(proof) = inspection.prove().await? else {
            return Ok(None);
        };
        let response = self
            .transport
            .exchange(handle, RequestFrame::RuntimeStatus(proof.request().clone()))
            .await?;
        proof.complete(response).map(Some)
    }
}

/// Opaque call-local operation. Only this backend can construct its fresh proof;
/// transports may opt in after taking exclusive ownership of a reusable stream.
pub struct NativeStatusInspection<'a> {
    backend: &'a AgentSandboxBackend,
    handle: &'a RuntimeHandle,
    request: &'a ProviderSandboxRequest,
    home: &'a HomeVolumeAttachment,
}

/// Not cloneable and never retained in transport state. Consumed by one reply.
pub(crate) struct NativeStatusProof {
    observation: RuntimeObservation,
    pod: crate::agent_sandbox::PodBinding,
    request: RuntimeStatusRequest,
}

impl NativeStatusInspection<'_> {
    pub(crate) fn shares_client(&self, client: &AgentSandboxClient) -> bool {
        self.backend.client.shares_runtime_authority(client)
    }

    pub(crate) fn handle(&self) -> &RuntimeHandle {
        self.handle
    }

    pub(crate) fn identity(&self) -> Result<(String, String)> {
        Ok((
            crate::hosted::deployment::provider_request_identity(self.request)?.sandbox_id,
            crate::provider_request_digest(self.request)?,
        ))
    }

    pub(crate) async fn prove(&self) -> Result<Option<NativeStatusProof>> {
        let (backend, handle, request, home) = (self.backend, self.handle, self.request, self.home);
        let (proof, policy) = tokio::join!(
            backend.client.native_runtime_proof(handle, request, home),
            backend.client.verify_native_runtime_policy(handle, home),
        );
        // Consume graph errors and the bootstrap fallback before policy errors,
        // preserving the original authority/error precedence of this operation.
        let proof = proof?;
        if !proof.matches(request, home)?
            || !backend.acknowledged_assignment(handle, proof.pod(), request)?
        {
            // An initial/replaced process must run the original bootstrap protocol.
            return Ok(None);
        }
        backend
            .client
            .complete_native_runtime_policy(home, proof.member(), policy)?;
        if !backend.acknowledged_assignment(handle, proof.pod(), request)? {
            return Err(AdapterError::Unavailable(
                "runtime assignment retired during inspection".into(),
            ));
        }
        backend.bind_transport(handle, request).await?;
        let observed = observation(proof.member())?;
        let identity = crate::hosted::deployment::provider_request_identity(request)?;
        let status_request = RuntimeStatusRequest {
            sandbox_id: identity.sandbox_id,
            generation: identity.generation,
            home_identity: home.identity()?,
        };
        Ok(Some(NativeStatusProof {
            observation: observed,
            pod: proof.pod().clone(),
            request: status_request,
        }))
    }
}

impl NativeStatusProof {
    pub(crate) fn pod(&self) -> &crate::agent_sandbox::PodBinding {
        &self.pod
    }

    pub(crate) fn request(&self) -> &RuntimeStatusRequest {
        &self.request
    }

    pub(crate) fn complete(self, response: ResponseFrame) -> Result<RuntimeInspection> {
        let ResponseFrame::RuntimeStatus(status) = response else {
            return Err(AdapterError::unexpected_reply("runtime status"));
        };
        status.validate_for(&self.request)?;
        Ok(RuntimeInspection {
            observation: self.observation,
            status: Some(status),
        })
    }
}
