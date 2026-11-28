//! Reuse only an acknowledged assignment to the same runtime container incarnation.
use super::*;
use crate::agent_sandbox::PodBinding;
use std::time::Duration;

pub(super) const MAX_ASSIGNMENTS: usize = 1024;

pub(super) struct Assignment {
    pub(super) pod: PodBinding,
    pub(super) request: ProviderSandboxRequest,
    pub(super) request_sha256: String,
    pub(super) file_read: tokio::sync::Mutex<super::file_read::FileReadBinding>,
}

impl PartialEq for Assignment {
    fn eq(&self, other: &Self) -> bool {
        self.pod == other.pod
            && self.request == other.request
            && self.request_sha256 == other.request_sha256
    }
}
impl Eq for Assignment {}

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
            file_read: Default::default(),
        }))
    }
}

impl AgentSandboxBackend {
    pub(super) async fn bind(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> Result<()> {
        if let Some(until) = request.funded_until {
            self.client
                .set_runtime_deadline(handle, request, until)
                .await?;
        }
        let mut timing = crate::agent_sandbox::provision_timing::ProvisionTiming::new();
        let pod = self.client.wait_ready_for_create(handle).await?;
        timing.completed("wait_ready");
        if let Some(until) = request.funded_until {
            // Readiness can consume the remaining window; never bind expired work.
            crate::agent_sandbox::funding::deadline(until)?;
        }
        let result = self.assign_and_bind(handle, &pod, request).await;
        timing.completed("assign_and_bind");
        result
    }

    pub(super) fn acknowledged_assignment(
        &self,
        handle: &RuntimeHandle,
        pod: &PodBinding,
        request: &ProviderSandboxRequest,
    ) -> Result<bool> {
        Ok(Assignment::for_request(pod, request)?
            .as_ref()
            .is_some_and(|candidate| {
                self.assignments
                    .lock()
                    .unwrap()
                    .get(handle)
                    .map(Arc::as_ref)
                    == Some(candidate)
            }))
    }

    pub(super) fn forget_assignment(&self, handle: &RuntimeHandle) {
        self.assignments.lock().unwrap().remove(handle);
        self.startup_breaker.reset(handle);
    }

    /// Adopts a native VCT runtime from its persisted home attachment rather
    /// than rediscovering that home through Kubernetes reads.
    pub(super) async fn adopt_persisted_home(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<RuntimeObservation>> {
        let object = timed(
            Stage::NativeIdentity,
            self.client
                .validate_native_request_with_home(handle, request, home),
        )
        .await?;
        self.bind_adopted(handle, request, &object, Some(home))
            .await?;
        // Binding may wait or deliver bootstrap. Its earlier observation cannot
        // authorize the returned runtime after either boundary completes.
        timed(
            Stage::NativeIdentity,
            self.refreshed_adoption_observation(handle, request, Some(home)),
        )
        .await
    }

    pub(super) async fn bind_adopted(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        observed: &DynamicObject,
        retained_home: Option<&HomeVolumeAttachment>,
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
            self.assignments
                .lock()
                .unwrap()
                .get(handle)
                .map(Arc::as_ref)
                == Some(candidate)
        });
        if acknowledged {
            // Policy is a live admission authority. Check it before binding
            // transport; the post-bind refresh cannot make an old bind safe.
            self.client.verify_explicit_runtime_policy(observed).await?;
            return self.bind_transport(handle, request).await;
        }
        if self.client.runtime_startup_breaker_enabled() {
            if let Some(home) = retained_home {
                self.recover_startup_hold(handle, request, home, &pod).await;
            }
        }
        self.assign_and_bind(handle, &pod, request).await
    }

    async fn recover_startup_hold(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
        pod: &PodBinding,
    ) {
        let Some(incarnation) = crate::agent_sandbox::startup_incarnation(pod) else {
            return;
        };
        if !self.startup_breaker.needs_recovery(handle, &incarnation)
            || self.assignments.lock().unwrap().contains_key(handle)
        {
            return;
        }
        let Ok(identity) = super::super::deployment::provider_request_identity(request) else {
            return;
        };
        let Some(history) = self.client.startup_history(pod).await else {
            return;
        };
        // Read the retained authority and the exact current binding again after
        // reading history. A stale generation or a replacement must not open a hold.
        let Ok(proof) = self
            .client
            .native_runtime_proof(handle, request, home)
            .await
        else {
            return;
        };
        if proof.pod() != pod || !proof.matches(request, home).unwrap_or(false) {
            return;
        }
        // A serving or assignment proof acquired during the read wins. The
        // breaker itself serializes concurrent observations and their deadline.
        let assignments = self.assignments.lock().unwrap();
        if !assignments.contains_key(handle) {
            self.startup_breaker.recover(
                handle,
                &incarnation,
                identity.sandbox_id,
                identity.generation,
                history,
            );
        }
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
        let identity = enabled
            .then(|| super::super::deployment::provider_request_identity(request).ok())
            .flatten();
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
        if let Some((receipt, confirmation)) = assigned {
            if let (Some(incarnation), Some(identity)) = (incarnation, identity) {
                self.startup_breaker.delivered(
                    handle,
                    incarnation,
                    identity.sandbox_id,
                    identity.generation,
                    std::time::SystemTime::now(),
                );
            }
            if let Some(candidate) = Assignment::for_request(pod, request)? {
                let mut assignments = self.assignments.lock().unwrap();
                if assignments.len() >= MAX_ASSIGNMENTS {
                    assignments.pop_first();
                }
                assignments.insert(handle.clone(), Arc::new(candidate));
            }
            if let Some(bootstrap) = confirmation {
                if let Err(error) = self
                    .client
                    .confirm_staging_startup_failure(handle, pod, bootstrap, &receipt)
                    .await
                {
                    tracing::warn!(%error, "staging startup failure confirmation did not complete");
                }
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
