//! One-operation native authority and process inspection.
use super::*;
use crate::hosted::inspection_timing;
use std::time::{Duration, Instant};

const ADOPTION_REFRESH_ATTEMPTS: usize = 3;

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
    /// Read the Sandbox again after readiness/assignment. The object used to
    /// authorize binding is deliberately not reused as the running observation:
    /// Kubernetes may have returned Pending immediately before the controller
    /// published Ready. Only a bounded Pending retry is allowed; identity and
    /// ownership failures remain fail-closed.
    pub(super) async fn refreshed_adoption_observation(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: Option<&HomeVolumeAttachment>,
    ) -> Result<Option<RuntimeObservation>> {
        let started = Instant::now();
        for attempt in 0..ADOPTION_REFRESH_ATTEMPTS {
            let observed = match home {
                Some(home) => Some(
                    self.client
                        .validate_native_request_with_home(handle, request, home)
                        .await?,
                ),
                None => self.client.validate_request(handle, request).await?,
            };
            let Some(object) = observed else {
                return Ok(None);
            };
            let observed = observation(&object)?;
            if observed.lifecycle.is_running() {
                self.client.verify_explicit_runtime_policy(&object).await?;
                return Ok(Some(observed));
            }
            if !observed.lifecycle.state.eq_ignore_ascii_case("pending") {
                return Ok(Some(observed));
            }
            if attempt + 1 < ADOPTION_REFRESH_ATTEMPTS {
                let delay = if started.elapsed() < Duration::from_secs(30) {
                    Duration::from_millis(250)
                } else {
                    Duration::from_millis(500)
                };
                tokio::time::sleep(delay).await;
            }
        }
        Err(AdapterError::Unavailable(
            "Sandbox readiness is still pending".into(),
        ))
    }

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
        if let Some(until) = request.funded_until {
            // Readiness can consume the remaining window; never bind expired work.
            crate::agent_sandbox::funding::deadline(until)?;
        }
        self.assign_and_bind(handle, &pod, request).await
    }

    pub(super) async fn inspect_native_context(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeInspection>> {
        self.inspect_native_context_after(
            handle,
            request,
            home,
            &crate::InspectionAdmission::ready(),
        )
        .await
    }

    pub(super) async fn inspect_native_context_after(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<RuntimeInspection>> {
        let result = self
            .inspect_native_context_inner(handle, request, home, admission)
            .await;
        if self.client.runtime_startup_breaker_enabled()
            && matches!(&result, Err(AdapterError::Unavailable(_)))
        {
            if let Some(failure) = self.startup_breaker.failure(handle) {
                return Err(AdapterError::RuntimeStartupHold(failure));
            }
        }
        result
    }

    async fn inspect_native_context_inner(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        admission: &crate::InspectionAdmission,
    ) -> Result<Option<RuntimeInspection>> {
        if !home.request.policy.is_native_vct() {
            return Ok(None);
        }
        let inspection = NativeStatusInspection {
            admission,
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
        let response = inspection_timing::measure(
            &home.request.sandbox_id,
            "native_status_exchange",
            self.transport
                .exchange(handle, RequestFrame::RuntimeStatus(proof.request().clone())),
        )
        .await?;
        inspection.complete(proof, response).await.map(Some)
    }
}

/// Opaque call-local operation. Only this backend can construct its fresh proof;
/// transports may opt in after taking exclusive ownership of a reusable stream.
pub struct NativeStatusInspection<'a> {
    admission: &'a crate::InspectionAdmission,
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
    pub(crate) async fn complete(
        &self,
        proof: NativeStatusProof,
        response: ResponseFrame,
    ) -> Result<RuntimeInspection> {
        let (backend, handle, request, home) = (self.backend, self.handle, self.request, self.home);
        // The captured reply must still belong to the exact graph and process
        // that authorized dispatch. These reads cannot bootstrap or rebind it.
        let current = inspection_timing::measure(
            &home.request.sandbox_id,
            "post_status_graph",
            timed(
                Stage::NativeIdentity,
                backend.client.native_runtime_proof(handle, request, home),
            ),
        )
        .await?;
        // This fresh policy read follows graph completion. An earlier policy
        // success cannot authorize a later graph after the definition changes.
        inspection_timing::measure(
            &home.request.sandbox_id,
            "post_status_policy",
            backend
                .client
                .verify_explicit_runtime_policy(current.member()),
        )
        .await?;
        if !current.matches(request, home)?
            || current.pod() != proof.pod()
            || observation(current.member())? != proof.observation
            || !backend.acknowledged_assignment(handle, current.pod(), request)?
        {
            return Err(AdapterError::Rejected(
                "native authority changed during status exchange".into(),
            ));
        }
        let inspected = proof.complete(response)?;
        backend.startup_breaker.reset(handle);
        Ok(inspected)
    }

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

    async fn read_authority(
        &self,
    ) -> (Result<crate::agent_sandbox::NativeRuntimeProof>, Result<()>) {
        let (backend, handle, request, home) = (self.backend, self.handle, self.request, self.home);
        tokio::join!(
            inspection_timing::measure(
                &home.request.sandbox_id,
                "native_authority_graph",
                backend.client.native_runtime_proof(handle, request, home),
            ),
            inspection_timing::measure(
                &home.request.sandbox_id,
                "native_runtime_policy",
                backend.client.verify_native_runtime_policy(handle, home),
            ),
        )
    }

    pub(crate) async fn prove(&self) -> Result<Option<NativeStatusProof>> {
        let (backend, handle, request, home) = (self.backend, self.handle, self.request, self.home);
        let (proof, policy) = self.read_authority().await;
        // Never carry a completed observation across an admission wait. The
        // common case hides slow native reads behind parent loading; the other
        // timing order repeats the reads after validation, retaining freshness.
        let (proof, policy) = if self.admission.is_ready() {
            (proof, policy)
        } else {
            self.admission.wait().await?;
            self.read_authority().await
        };
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
