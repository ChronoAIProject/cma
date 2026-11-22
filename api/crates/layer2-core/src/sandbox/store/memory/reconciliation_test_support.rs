use super::*;

pub(super) fn loss_request(value: &Sandbox) -> SandboxRuntimeLossRequest {
    SandboxRuntimeLossRequest {
        sandbox_id: value.id.clone(),
        expected_revision: value.revision,
        expected_runtime: value.runtime.clone(),
        native_retirement_finality: false,
        retain_runtime: false,
        interruption_cause: Default::default(),
        observed_at: NOW.into(),
    }
}
