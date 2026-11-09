//! Reuse only an acknowledged assignment to the same runtime container incarnation.
use super::*;
use crate::agent_sandbox::PodBinding;

const MAX_ASSIGNMENTS: usize = 1024;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Assignment {
    pub(super) pod: PodBinding,
    pub(super) request: ProviderSandboxRequest,
    request_sha256: String,
}

impl Assignment {
    fn for_request(pod: &PodBinding, request: &ProviderSandboxRequest) -> Result<Option<Self>> {
        if pod
            .runtime_container_id
            .as_deref()
            .is_none_or(str::is_empty)
            || pod.runtime_restart_count.is_none_or(|count| count < 0)
        {
            return Ok(None);
        }
        Ok(Some(Self {
            pod: pod.clone(),
            request: request.clone(),
            request_sha256: crate::provider_request_digest(request)?,
        }))
    }
}

impl AgentSandboxBackend {
    pub(super) fn acknowledged_assignment(
        &self,
        handle: &RuntimeHandle,
        pod: &PodBinding,
        request: &ProviderSandboxRequest,
    ) -> Result<bool> {
        Ok(Assignment::for_request(pod, request)?
            .as_ref()
            .is_some_and(|candidate| {
                self.assignments.lock().unwrap().get(handle) == Some(candidate)
            }))
    }

    pub(super) fn forget_assignment(&self, handle: &RuntimeHandle) {
        self.assignments.lock().unwrap().remove(handle);
    }

    pub(super) async fn bind_adopted(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        observed: &DynamicObject,
    ) -> Result<()> {
        // The caller has freshly validated the original Claim/Sandbox/PVC and
        // complete request. A receipt never replaces that current authority proof.
        let pod = timed(
            Stage::NativeReadiness,
            self.client.wait_ready(handle, Duration::from_secs(180)),
        )
        .await?;
        let candidate = Assignment::for_request(&pod, request)?;
        let acknowledged = candidate.as_ref().is_some_and(|candidate| {
            self.assignments.lock().unwrap().get(handle) == Some(candidate)
        });
        if acknowledged {
            // Policy is a live admission authority. Check it before binding
            // transport; the post-bind refresh cannot make an old bind safe.
            self.client.verify_explicit_runtime_policy(observed).await?;
            return self.bind_transport(handle, request).await;
        }
        self.assign_and_bind(handle, &pod, request).await
    }

    pub(super) async fn assign_and_bind(
        &self,
        handle: &RuntimeHandle,
        pod: &PodBinding,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        self.forget_assignment(handle);
        let assigned = timed(
            Stage::NativeAssignment,
            self.client.assign_pool_runtime(handle, pod, request),
        )
        .await?;
        if assigned.is_some() {
            if let Some(candidate) = Assignment::for_request(pod, request)? {
                let mut assignments = self.assignments.lock().unwrap();
                if assignments.len() >= MAX_ASSIGNMENTS {
                    assignments.pop_first();
                }
                assignments.insert(handle.clone(), candidate);
            }
        }
        self.bind_transport(handle, request).await
    }

    pub(super) async fn bind_transport(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        let identity = super::super::deployment::provider_request_identity(request)?;
        self.transport
            .bind_native_runtime(
                handle,
                &identity.sandbox_id,
                &crate::provider_request_digest(request)?,
            )
            .await
    }
}
