use super::*;
use crate::ingress_budget::AdmissionRefusal;

impl RuntimeBackends {
    pub(super) fn admitted_backend<T>(
        &self,
        handle: &RuntimeHandle,
        request: &OwnedWork<T>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<&dyn HostedBackend, AdmittedError> {
        if !request.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "routed request ownership",
            }
            .into());
        }
        // Selection uses the recorded handle, including when allocation defaults
        // change. The legacy selector allocates its missing-provider error.
        let backend = if handle.0.starts_with("agent-sandbox:") {
            &self.agent_sandbox
        } else {
            &self.opensandbox
        };
        backend.as_deref().ok_or(AdmittedError::Unavailable(
            "recorded runtime provider is not configured",
        ))
    }
}
