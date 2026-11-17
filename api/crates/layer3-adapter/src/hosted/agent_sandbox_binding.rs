//! Reuse only an acknowledged assignment to the same runtime container incarnation.
use super::*;
use crate::agent_sandbox::PodBinding;
use std::time::Duration;

pub(super) const MAX_ASSIGNMENTS: usize = 1024;

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
        self.startup_breaker.reset(handle);
    }

    pub(super) async fn bind_adopted(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        observed: &DynamicObject,
    ) -> Result<()> {
        // The caller has freshly validated the original Claim/Sandbox/PVC and
        // complete request. A receipt never replaces that current authority proof.
        if self.client.runtime_startup_breaker_enabled()
            && self.startup_breaker.failure(handle).is_some()
        {
            if let Some(hold) = self
                .startup_hold_before_wait(handle, request, observed)
                .await
            {
                return Err(hold);
            }
        }
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

    async fn startup_hold_before_wait(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        observed: &DynamicObject,
    ) -> Option<AdapterError> {
        // Optional evidence must never replace the ordinary authority/readiness
        // path with a new error. An acknowledged delivery uses that path too.
        let (boot, ready) = self.client.startup_boot(observed).await?;
        if let Some(pod) = &ready {
            if self.acknowledged_assignment(handle, pod, request).ok()? {
                return None;
            }
        }
        self.startup_breaker.reset_boot(handle, &boot);
        if let Some(pod) = &ready {
            if let Some(current) = crate::agent_sandbox::startup_incarnation(pod) {
                if let Some(delivered) = self.startup_breaker.evidence_needed(handle, &current) {
                    let evidence = self.client.startup_termination(pod).await?;
                    self.startup_breaker.observe(handle, &delivered, evidence);
                }
            }
        }
        self.startup_breaker.hold_before_wait(handle)
    }

    pub(super) async fn assign_and_bind(
        &self,
        handle: &RuntimeHandle,
        pod: &PodBinding,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        let enabled = self.client.runtime_startup_breaker_enabled();
        let sandbox_id = enabled
            .then(|| super::super::deployment::provider_request_identity(request).ok())
            .flatten()
            .map(|identity| identity.sandbox_id);
        let incarnation = enabled
            .then(|| crate::agent_sandbox::startup_incarnation(pod))
            .flatten();
        if enabled {
            self.startup_breaker
                .reset_boot(handle, crate::agent_sandbox::startup_boot_identity(pod));
            if let Some(current) = &incarnation {
                if let Some(delivered) = self.startup_breaker.evidence_needed(handle, current) {
                    if let Some(evidence) = self.client.startup_termination(pod).await {
                        self.startup_breaker.observe(handle, &delivered, evidence);
                    }
                }
            }
            self.startup_breaker.admit(handle)?;
        }
        self.assignments.lock().unwrap().remove(handle);
        let assigned = timed(
            Stage::NativeAssignment,
            self.client.assign_pool_runtime(handle, pod, request),
        )
        .await?;
        if assigned.is_some() {
            if let (Some(incarnation), Some(sandbox_id)) = (incarnation, sandbox_id) {
                self.startup_breaker.delivered(
                    handle,
                    incarnation,
                    sandbox_id,
                    std::time::SystemTime::now(),
                );
            }
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

#[cfg(test)]
#[path = "startup_headroom_tests.rs"]
mod startup_headroom_tests;
